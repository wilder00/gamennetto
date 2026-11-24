//! Release helpers, run with `cargo run -p xtask -- <command>`.
//!
//!   keygen [FILE]   Make the update signing key (once, by the maintainer).
//!                   Writes FILE (private, password-protected) and FILE.pub.
//!                   Default FILE: ~/.gamennetto/update.key
//!   sign FILE       Writes FILE.sig. Reads the private key from the
//!                   environment: GAMENNETTO_SIGNING_KEY (the key file's
//!                   content) and GAMENNETTO_SIGNING_KEY_PASSWORD.
//!
//! Keys and signatures are minisign, each stored as base64 of minisign's
//! text (the format `gamennetto update` checks).

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(args.get(1).map(PathBuf::from)),
        Some("sign") => sign(PathBuf::from(args.get(1).context("usage: xtask sign FILE")?)),
        _ => bail!("usage: cargo run -p xtask -- keygen [FILE] | sign FILE"),
    }
}

fn keygen(file: Option<PathBuf>) -> Result<()> {
    let file = match file {
        Some(f) => f,
        None => {
            let home =
                std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).context("no home folder")?;
            PathBuf::from(home).join(".gamennetto").join("update.key")
        }
    };
    let pub_file = PathBuf::from(format!("{}.pub", file.display()));
    if file.exists() || pub_file.exists() {
        bail!("{} already exists; not overwriting a signing key", file.display());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // minisign asks for the password (twice) on the terminal, unless it is
    // given in GAMENNETTO_SIGNING_KEY_PASSWORD (for scripts and tests).
    let password = std::env::var("GAMENNETTO_SIGNING_KEY_PASSWORD").ok();
    let kp = minisign::KeyPair::generate_encrypted_keypair(password)?;
    let sk = B64.encode(kp.sk.to_box(Some("gamennetto update key"))?.to_string());
    let pk = B64.encode(kp.pk.to_box()?.to_string());
    std::fs::write(&file, &sk)?;
    std::fs::write(&pub_file, &pk)?;
    println!("Private key: {}  (back it up; never share it)", file.display());
    println!("Public key:  {}", pub_file.display());
    println!("\nOn GitHub (Settings > Secrets and variables > Actions):");
    println!("  secret   GAMENNETTO_SIGNING_KEY           = content of {}", file.display());
    println!("  secret   GAMENNETTO_SIGNING_KEY_PASSWORD  = the password you just typed");
    println!("  variable UPDATE_PUBKEY                    = {pk}");
    Ok(())
}

fn sign(file: PathBuf) -> Result<()> {
    let key = std::env::var("GAMENNETTO_SIGNING_KEY").context("GAMENNETTO_SIGNING_KEY is not set")?;
    let password = std::env::var("GAMENNETTO_SIGNING_KEY_PASSWORD").unwrap_or_default();
    let text = String::from_utf8(B64.decode(key.trim()).context("GAMENNETTO_SIGNING_KEY is not base64")?)?;
    let sk = minisign::SecretKeyBox::from_string(&text)?
        .into_secret_key(Some(password))
        .context("could not open the signing key (wrong password?)")?;
    let data = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let sig = minisign::sign(None, &sk, std::io::Cursor::new(&data), Some(&format!("file:{name}")), None)?;
    let out = PathBuf::from(format!("{}.sig", file.display()));
    std::fs::write(&out, B64.encode(sig.to_string()))?;
    println!("Signed: {}", out.display());
    Ok(())
}
