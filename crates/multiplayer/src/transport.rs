//! QUIC/TLS 1.3 provided by Quinn/rustls, no custom encryption. Clients
//! trust only an explicitly supplied relay certificate and validate its name.

use crate::protocol::MAX_CONTROL;
use anyhow::{Result, ensure};
use quinn::{Endpoint, RecvStream, SendStream, TransportConfig, VarInt};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub const SERVER_NAME: &str = "cod4rw-relay";
const ALPN: &[u8] = b"cod4rw-mp/1";

fn transport(server: bool) -> TransportConfig {
    let mut t = TransportConfig::default();
    t.max_concurrent_bidi_streams(VarInt::from_u32(if server { 1 } else { 0 }))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .stream_receive_window(VarInt::from_u32(1024))
        .receive_window(VarInt::from_u32(4096))
        .send_window(16 * 1024)
        .datagram_receive_buffer_size(Some(16 * 1024))
        .datagram_send_buffer_size(16 * 1024)
        .max_idle_timeout(Some(VarInt::from_u32(15_000).into()))
        .keep_alive_interval(Some(Duration::from_secs(3)))
        .allow_spin(false);
    t
}

pub fn server_endpoint(bind: SocketAddr, cert: Vec<u8>, key: Vec<u8>) -> Result<Endpoint> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert)], PrivatePkcs8KeyDer::from(key).into())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport(true)));
    // No client 0-RTT: room creation/join cannot be replayed as early data.
    Endpoint::server(config, bind).map_err(Into::into)
}

pub fn client_endpoint(bind: SocketAddr, cert: Vec<u8>) -> Result<Endpoint> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(cert))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let config = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let mut config = quinn::ClientConfig::new(Arc::new(config));
    config.transport_config(Arc::new(transport(false)));
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

pub async fn read_frame(stream: &mut RecvStream) -> Result<Vec<u8>> {
    let mut size = [0; 2];
    stream.read_exact(&mut size).await?;
    let size = u16::from_le_bytes(size) as usize;
    ensure!(size > 0 && size <= MAX_CONTROL, "invalid control frame length");
    let mut data = vec![0; size];
    stream.read_exact(&mut data).await?;
    Ok(data)
}
pub async fn write_frame(stream: &mut SendStream, data: &[u8]) -> Result<()> {
    ensure!(!data.is_empty() && data.len() <= MAX_CONTROL, "invalid control frame length");
    stream.write_all(&(data.len() as u16).to_le_bytes()).await?;
    stream.write_all(data).await?;
    Ok(())
}

/// Development identity. Distribute the public certificate out of band;
/// never retrieve it from an untrusted server and automatically trust it.
pub fn generate_identity() -> Result<(Vec<u8>, Vec<u8>)> {
    let identity = rcgen::generate_simple_self_signed(vec![SERVER_NAME.into()])?;
    Ok((identity.cert.der().to_vec(), identity.signing_key.serialize_der()))
}
