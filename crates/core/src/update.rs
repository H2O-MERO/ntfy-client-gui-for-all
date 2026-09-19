use futures_util::StreamExt;
use ntfy_pusher_ipc::UpdateStatus;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;
use tokio::{fs::File, io::AsyncWriteExt};

const DEFAULT_REPOSITORY: &str = "H2O-MERO/ntfy-client-gui-for-all";

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("update request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("update file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid version: {0}")]
    Version(#[from] semver::Error),
    #[error("release response did not contain a supported artifact")]
    NoAsset,
    #[error("release response did not contain a checksum for the artifact")]
    NoChecksum,
    #[error("invalid SHA-256 checksum file")]
    InvalidChecksum,
    #[error("downloaded update did not match its SHA-256 checksum")]
    ChecksumMismatch,
}

#[derive(Debug, Clone)]
pub struct VerifiedDownload {
    pub path: PathBuf,
    pub sha256: String,
    pub release_url: String,
}

#[derive(Clone)]
pub struct UpdateChecker {
    client: reqwest::Client,
    repository: String,
}

impl UpdateChecker {
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::for_repository(DEFAULT_REPOSITORY)
    }

    pub fn for_repository(repository: impl Into<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("ntfy-pusher/", env!("CARGO_PKG_VERSION")))
                .build()?,
            repository: repository.into(),
        })
    }

    pub async fn check(&self, current: &Version) -> Result<UpdateStatus, UpdateError> {
        let release = self.latest_release().await?;
        let latest = parse_tag(&release.tag_name)?;
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == platform_asset_name());
        let checksum = asset.and_then(|asset| {
            release
                .assets
                .iter()
                .find(|candidate| candidate.name == format!("{}.sha256", asset.name))
        });
        Ok(UpdateStatus {
            current_version: current.to_string(),
            latest_version: Some(latest.to_string()),
            available: latest > *current,
            release_url: Some(release.html_url),
            release_notes: release.body,
            verified_asset_available: asset.is_some() && checksum.is_some(),
            automatic_install_supported: false,
        })
    }

    /// Downloads only an artifact with a matching sidecar SHA-256 checksum.
    pub async fn download_verified(
        &self,
        destination: &Path,
    ) -> Result<VerifiedDownload, UpdateError> {
        let release = self.latest_release().await?;
        let asset_name = platform_asset_name();
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or(UpdateError::NoAsset)?;
        let checksum_asset = release
            .assets
            .iter()
            .find(|candidate| candidate.name == format!("{}.sha256", asset.name))
            .ok_or(UpdateError::NoChecksum)?;
        let checksum_text = self
            .client
            .get(&checksum_asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let expected = parse_checksum(&checksum_text)?;

        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temporary = destination.with_extension("download");
        let response = self
            .client
            .get(&asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut file = File::create(&temporary).await?;
        let mut digest = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            digest.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        let actual = format!("{:x}", digest.finalize());
        if actual != expected {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(UpdateError::ChecksumMismatch);
        }
        #[cfg(windows)]
        if destination.exists() {
            tokio::fs::remove_file(destination).await?;
        }
        tokio::fs::rename(&temporary, destination).await?;
        Ok(VerifiedDownload {
            path: destination.to_owned(),
            sha256: actual,
            release_url: release.html_url,
        })
    }

    async fn latest_release(&self) -> Result<GitHubRelease, UpdateError> {
        let url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            self.repository
        );
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

fn parse_tag(tag: &str) -> Result<Version, semver::Error> {
    Version::parse(tag.trim().trim_start_matches(['v', 'V']))
}

fn platform_asset_name() -> String {
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!(
        "ntfy-pusher-{}-{}.{}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        extension
    )
}

fn parse_checksum(contents: &str) -> Result<String, UpdateError> {
    let candidate = contents
        .split_whitespace()
        .next()
        .ok_or(UpdateError::InvalidChecksum)?
        .to_ascii_lowercase();
    if candidate.len() == 64 && candidate.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(candidate)
    } else {
        Err(UpdateError::InvalidChecksum)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v_prefixed_semver() {
        assert_eq!(parse_tag("v1.2.3").unwrap(), Version::new(1, 2, 3));
    }

    #[test]
    fn accepts_sha256sum_format_only() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_checksum(&format!("{hash}  artifact.zip\n")).unwrap(),
            hash
        );
        assert!(parse_checksum("not-a-hash file").is_err());
    }
}
