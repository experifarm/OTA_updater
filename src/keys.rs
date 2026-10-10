//! Ed25519 key generation and hex key files.

use anyhow::{Context, Result, bail};
use ed25519_dalek::{SigningKey, VerifyingKey};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SECRET_FILE: &str = "ota_signing.key";
pub const PUBLIC_FILE: &str = "ota_signing.pub";

/// Fill `buf` from the operating system CSPRNG.
pub fn os_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| anyhow::anyhow!("OS random number generator failed: {e}"))
}

pub fn random_u64() -> Result<u64> {
    let mut b = [0u8; 8];
    os_random(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// Generate a fresh signing key from OS randomness.
pub fn generate() -> Result<SigningKey> {
    let mut seed = [0u8; 32];
    os_random(&mut seed)?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Write `<dir>/ota_signing.key` (mode 0600) and `<dir>/ota_signing.pub` as hex.
pub fn save_keypair(dir: &Path, key: &SigningKey, force: bool) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let secret = dir.join(SECRET_FILE);
    let public = dir.join(PUBLIC_FILE);
    if !force && (secret.exists() || public.exists()) {
        bail!(
            "{} or {} already exists (use --force to overwrite)",
            secret.display(),
            public.display()
        );
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&secret)
        .with_context(|| format!("writing {}", secret.display()))?;
    writeln!(f, "{}", crate::to_hex(key.as_bytes()))?;
    fs::write(&public, format!("{}\n", crate::to_hex(key.verifying_key().as_bytes())))
        .with_context(|| format!("writing {}", public.display()))?;
    Ok((secret, public))
}

fn read_hex32(path: &Path) -> Result<[u8; 32]> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let bytes = crate::from_hex(&text).with_context(|| format!("{} is not valid hex", path.display()))?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("{}: expected 32 bytes, got {}", path.display(), v.len()))
}

pub fn load_signing_key(path: &Path) -> Result<SigningKey> {
    Ok(SigningKey::from_bytes(&read_hex32(path)?))
}

pub fn load_verifying_key(path: &Path) -> Result<VerifyingKey> {
    VerifyingKey::from_bytes(&read_hex32(path)?)
        .map_err(|e| anyhow::anyhow!("{}: invalid Ed25519 public key: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let key = generate().unwrap();
        let (secret, public) = save_keypair(dir.path(), &key, false).unwrap();
        assert!(
            save_keypair(dir.path(), &key, false).is_err(),
            "must not overwrite silently"
        );
        assert_eq!(load_signing_key(&secret).unwrap().to_bytes(), key.to_bytes());
        assert_eq!(load_verifying_key(&public).unwrap(), key.verifying_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_ne!(generate().unwrap().to_bytes(), key.to_bytes());
    }
}
