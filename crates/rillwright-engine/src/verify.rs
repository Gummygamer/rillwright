//! Startup checks: the model file must match the pinned SHA-256 and the linked
//! llama.cpp fork must be the pinned commit. Both fail with a clear error.

use std::{fs::File, io::Read, path::Path};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// `NVIDIA-Nemotron-Labs-Dory1.0-4B-Q5_K_M.gguf`
pub const PINNED_MODEL_SHA256: &str =
    "96080be49ffd67b916018b7bdef8c484762576fb52780e10ab98ac4978a660e0";
/// The llama.cpp PR #30306 head that the Dory runtime was tested against.
pub const PINNED_FORK_COMMIT: &str = "1c3cdd23787750814aa21a070dc4d6dc451af871";

/// Names the SHA-256 of a model that is deliberately not the pinned file, such as a copy with edited
/// weights (see `wsp/`). The file must still match the value given, so a typo or a stale path fails.
pub const MODEL_SHA256_OVERRIDE_ENV: &str = "RILLWRIGHT_MODEL_SHA256";

/// The checksum the model file must have: the pin, unless the override variable names another.
pub fn expected_model_sha256() -> String {
    match std::env::var(MODEL_SHA256_OVERRIDE_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_ascii_lowercase(),
        _ => PINNED_MODEL_SHA256.to_string(),
    }
}

/// Streams the file through SHA-256 and compares it with the expected checksum.
pub fn verify_model(path: &Path) -> Result<()> {
    let actual = sha256_file(path)?;
    let expected = expected_model_sha256();
    if actual != expected {
        return Err(Error::Checksum {
            path: path.to_path_buf(),
            expected,
            actual,
        });
    }
    Ok(())
}

/// Checks the commit the linked `libllama` was built from.
pub fn verify_fork() -> Result<()> {
    let built = rillwright_sys::FORK_COMMIT;
    if built != PINNED_FORK_COMMIT {
        return Err(Error::ForkCommit {
            built: built.to_string(),
            pinned: PINNED_FORK_COMMIT.to_string(),
        });
    }
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let io = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = File::open(path).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
