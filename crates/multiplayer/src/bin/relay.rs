use anyhow::{Context, Result, bail, ensure};
use cod4rw_multiplayer::{relay::Relay, transport};
use std::{fs::OpenOptions, io::Write, net::SocketAddr, path::Path};

fn create_file(path: &Path, data: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(data)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("identity") => {
            ensure!(args.len() == 2, "usage: cod4rw-relay identity <new-directory>");
            let dir = Path::new(&args[1]);
            // Refuse overwrite and a nonempty existing directory. The private
            // key is never printed, and is never put into an invite.
            std::fs::create_dir(dir).context("identity directory must not already exist")?;
            let (cert, key) = transport::generate_identity()?;
            create_file(&dir.join("relay.der"), &cert)?;
            create_file(&dir.join("relay-key.der"), &key)?;
            println!("Created relay.der (public trust certificate) and relay-key.der (private key).");
        }
        Some("serve") => {
            ensure!(
                (2..=3).contains(&args.len()),
                "usage: cod4rw-relay serve <identity-directory> [bind-address:port]"
            );
            let dir = Path::new(&args[1]);
            let bind: SocketAddr = args.get(2).map(String::as_str).unwrap_or("127.0.0.1:28970").parse()?;
            let endpoint = transport::server_endpoint(
                bind,
                std::fs::read(dir.join("relay.der"))?,
                std::fs::read(dir.join("relay-key.der"))?,
            )?;
            println!("Relay listening on {}. Private rooms; no peer addresses are advertised.", endpoint.local_addr()?);
            // Testing host elections on one PC: count every host this much
            // further from the relay than it is.
            let relay = match std::env::var("COD4RW_RELAY_HOST_PENALTY_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
                Some(ms) => Relay::with_host_penalty(std::time::Duration::from_millis(ms)),
                None => Relay::new(),
            };
            relay.run(endpoint).await?;
        }
        Some("list") => {
            ensure!((2..=3).contains(&args.len()), "usage: cod4rw-relay list <relay.der> [address:port]");
            let address: SocketAddr = args.get(2).map(String::as_str).unwrap_or("127.0.0.1:28970").parse()?;
            let mut browser = cod4rw_multiplayer::client::Browser::connect(address, std::fs::read(&args[1])?).await?;
            let (total, lobbies) = browser.list(Default::default()).await?;
            println!("{total} public lobbies ({} ms away)", browser.rtt().as_millis());
            for l in lobbies {
                let x = &l.listing;
                println!(
                    "{:016x}  {:5} {:16} {:2}/{:2} +{:2} bots  {:4} ms  {:?}{}  {}",
                    l.room,
                    x.mode,
                    x.map,
                    l.humans,
                    x.max,
                    x.bots,
                    l.ping,
                    x.state,
                    if l.open() { "" } else { " (closed)" },
                    x.region
                );
            }
        }
        _ => bail!("usage: cod4rw-relay identity <new-directory> | serve <identity-directory> [bind-address:port] | list <relay.der> [address:port]"),
    }
    Ok(())
}
