//! `gamennetto devices`: the PCs this PC has approved.

use anyhow::Result;
use gamennetto_net::{data_dir, Fingerprint, Identity, TrustStore};

#[derive(clap::Subcommand)]
pub enum Action {
    /// List approved PCs (the default).
    List,
    /// Forget a PC, by name or by the start of its fingerprint. It will need
    /// to be approved again (code or number check) to connect.
    Remove {
        /// Name (e.g. LEGION) or fingerprint start (e.g. 3f2a91c0).
        name: String,
    },
}

pub fn run(action: Option<Action>) -> Result<()> {
    match action.unwrap_or(Action::List) {
        Action::List => list(),
        Action::Remove { name } => {
            let mut t = TrustStore::open()?;
            let n = t.remove(&name);
            t.save()?;
            if n == 0 {
                println!("No approved PC matches '{name}'. See `gamennetto devices`.");
            } else {
                println!("Forgot {n} entr{}. It must be approved again to connect.", if n == 1 { "y" } else { "ies" });
            }
            Ok(())
        }
    }
}

fn list() -> Result<()> {
    let me = Identity::load_or_create(&data_dir().join("identity"))?;
    let t = TrustStore::open()?;
    println!("This PC's fingerprint: {}\n", me.fingerprint);
    let show = |title: &str, list: &[gamennetto_net::identity::Device]| {
        println!("{title}");
        if list.is_empty() {
            println!("  (none)");
        }
        for d in list {
            let fp = Fingerprint::from_hex(&d.fingerprint).map(|f| f.short()).unwrap_or_else(|_| d.fingerprint.clone());
            println!("  - {:<20} {fp}   approved {}", d.name, d.added);
        }
        println!();
    };
    show("PCs allowed to watch this one (viewers):", &t.viewers);
    show("PCs this one can watch (hosts):", &t.hosts);
    println!("Remove one with: gamennetto devices remove NAME");
    Ok(())
}
