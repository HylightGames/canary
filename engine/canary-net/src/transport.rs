//! Canary-owned transport boundary and the default QUIC adapter.
//!
//! [`NetTransport`] is the trait gameplay and session code programs against:
//! connect, accept, and exactly one reliable ordered byte stream per
//! connection (ADR 0027, point 2). No `quinn` type appears in any public
//! signature here — [`QuinnTransport`] wraps the endpoint and streams in
//! private fields behind [`NetSend`] / [`NetRecv`]. Swapping the backend
//! (raw UDP, loopback mock, a second QUIC lane) means a new implementor, not
//! new call sites.
//!
//! [`QuinnTransport`] constructors take DER bytes, not `rustls` types, for
//! the same reason: certificate and key material cross the boundary as data,
//! and all `quinn`/`rustls` usage stays inside this module.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::error::NetError;
use crate::frame::{decode_frame_len, encode_frame, FRAME_HEADER_LEN};
use crate::limits::NetLimits;

/// ALPN identifier negotiated on every Canary QUIC connection.
///
/// Both ends set it; a peer offering anything else fails the TLS handshake
/// before any Canary byte flows. Bumping the wire protocol incompatibly
/// means a new ALPN string alongside the new [`crate::ids::ProtocolVersion`].
pub const QUIC_ALPN_CANARY_1: &[u8] = b"canary-1";

/// Sending half of one reliable ordered stream.
///
/// The returned futures are `Send`: session drivers may move them across
/// threads (e.g. `tokio::spawn`) without giving up the trait boundary.
pub trait NetSend: Send {
    /// Frames `body` (length-prefix + bytes) and sends it in order.
    ///
    /// Rejects over-limit bodies before writing anything.
    fn send_frame(
        &mut self,
        body: &[u8],
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<(), NetError>> + Send + '_;

    /// Gracefully closes the sending direction.
    fn finish(&mut self) -> impl std::future::Future<Output = Result<(), NetError>> + Send + '_;
}

/// Receiving half of one reliable ordered stream.
pub trait NetRecv: Send {
    /// Receives one framed body.
    ///
    /// Reads the 4-byte prefix, gates it against `limits` before allocating,
    /// then reads exactly the claimed body. Never `read_to_end` on
    /// untrusted bytes.
    fn recv_frame(
        &mut self,
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, NetError>> + Send + '_;
}

/// Canary-owned transport interface: connect outward, accept inward, speak
/// over one reliable ordered stream per connection.
pub trait NetTransport: Send + Sync {
    /// Sending stream half for this backend.
    type Send: NetSend;
    /// Receiving stream half for this backend.
    type Recv: NetRecv;

    /// Connects to `addr`, verifying the server against this endpoint's trust
    /// policy under the given SNI `server_name`, and opens the session stream.
    fn connect(
        &self,
        addr: SocketAddr,
        server_name: &str,
    ) -> impl std::future::Future<Output = Result<(Self::Send, Self::Recv), NetError>> + Send + '_;

    /// Accepts the next inbound connection and its session stream.
    ///
    /// This profile has exactly one stream per connection, so connection
    /// acceptance and stream acceptance are one step. It completes once the
    /// peer opens the session stream — in the current message pattern the
    /// client speaks first, so the server's `accept` resolves when the
    /// client's first frame arrives (QUIC buffers it; no rendezvous is
    /// needed beyond that). Connection acceptance and stream acceptance
    /// stay fused in this profile: the handshake vocabulary kept the
    /// client-speaks-first pattern, so no split was needed.
    fn accept(
        &self,
    ) -> impl std::future::Future<Output = Result<(Self::Send, Self::Recv), NetError>> + Send + '_;

    /// Local address this endpoint is bound to.
    fn local_addr(&self) -> Result<SocketAddr, NetError>;
}

/// Maps any displayable backend failure to the unclassified fallback.
///
/// Structured per-operation variants ([`NetError::TransportConnect`] and
/// friends) are preferred at every fallible call site below; this fallback
/// remains for local setup failures (endpoint binding, certificate
/// configuration) that are neither a dial, an accept, nor stream IO.
fn transport_error(error: impl std::fmt::Display) -> NetError {
    NetError::Transport(error.to_string())
}

/// Maps a dial/establishment backend failure without leaking backend types.
fn connect_error(error: impl std::fmt::Display) -> NetError {
    NetError::TransportConnect {
        detail: error.to_string(),
    }
}

/// Maps an inbound-accept backend failure without leaking backend types.
fn accept_error(error: impl std::fmt::Display) -> NetError {
    NetError::TransportAccept {
        detail: error.to_string(),
    }
}

/// Default QUIC adapter over `quinn` 0.11.
///
/// The endpoint and its streams stay private; construction and use go through
/// [`NetTransport`], [`NetSend`], and [`NetRecv`], whose signatures name no
/// third-party type.
///
/// Constructors require an active Tokio runtime: `quinn` binds its UDP
/// socket through Tokio. Call them from async code (or any context holding a
/// runtime handle), not from bare synchronous setup.
pub struct QuinnTransport {
    /// The QUIC endpoint. Server mode serves with the configured identity;
    /// client mode dials with the pinned trust root.
    endpoint: quinn::Endpoint,
}

impl QuinnTransport {
    /// Binds a server endpoint on `addr` serving `certificate_der` (DER,
    /// single-certificate chain) with `private_key_der` (DER, PKCS#8).
    ///
    /// The certificate is transport encryption material, not application
    /// identity: per ADR 0027 point 9, QUIC TLS protects the bytes but does
    /// not decide who the peer is. Production identity, provisioning, and
    /// abuse policy are separate, later work.
    pub fn server(
        addr: SocketAddr,
        certificate_der: &[u8],
        private_key_der: &[u8],
    ) -> Result<Self, NetError> {
        use quinn::crypto::rustls::QuicServerConfig;
        use quinn::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        let certificate = CertificateDer::from(certificate_der.to_vec());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
        let mut tls = quinn::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], private_key)
            .map_err(transport_error)?;
        tls.alpn_protocols = vec![QUIC_ALPN_CANARY_1.to_vec()];
        // `quinn` speaks TLS through its own `Quic*Config` wrappers, which
        // enforce the QUIC-mandated TLS 1.3 profile at conversion time.
        let quic_tls = QuicServerConfig::try_from(tls).map_err(transport_error)?;
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_tls));

        let endpoint = quinn::Endpoint::server(server_config, addr).map_err(transport_error)?;
        Ok(Self { endpoint })
    }

    /// Binds a client endpoint trusting exactly `pinned_server_certificate_der`
    /// (DER) and nothing else.
    ///
    /// Proof-only peer policy: the client pins the expected server
    /// certificate instead of consulting system roots, which is sufficient
    /// for the separate-process loopback proof and is habitually wrong for a
    /// public deployment. No production authentication may be inferred from
    /// this (ADR 0027, point 9).
    pub fn client(pinned_server_certificate_der: &[u8]) -> Result<Self, NetError> {
        use quinn::crypto::rustls::QuicClientConfig;
        use quinn::rustls::pki_types::CertificateDer;

        let mut roots = quinn::rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(pinned_server_certificate_der.to_vec()))
            .map_err(transport_error)?;
        let mut tls = quinn::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![QUIC_ALPN_CANARY_1.to_vec()];
        let quic_tls = QuicClientConfig::try_from(tls).map_err(transport_error)?;

        let mut endpoint = quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))
            .map_err(transport_error)?;
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quic_tls)));
        Ok(Self { endpoint })
    }
}

impl NetTransport for QuinnTransport {
    type Send = QuinnSendStream;
    type Recv = QuinnRecvStream;

    fn connect(
        &self,
        addr: SocketAddr,
        server_name: &str,
    ) -> impl std::future::Future<Output = Result<(Self::Send, Self::Recv), NetError>> + Send + '_
    {
        // The SNI name is copied so the future borrows only `self`; callers
        // keep natural lifetimes for `server_name`.
        let server_name = server_name.to_owned();
        async move {
            let connection = self
                .endpoint
                .connect(addr, &server_name)
                .map_err(connect_error)?
                .await
                .map_err(connect_error)?;
            let (send, recv) = connection.open_bi().await.map_err(connect_error)?;
            Ok((
                QuinnSendStream { inner: send },
                QuinnRecvStream { inner: recv },
            ))
        }
    }

    async fn accept(&self) -> Result<(Self::Send, Self::Recv), NetError> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| NetError::TransportAccept {
                detail: "endpoint closed".to_string(),
            })?;
        let connecting = incoming.accept().map_err(accept_error)?;
        let connection = connecting.await.map_err(accept_error)?;
        let (send, recv) = connection.accept_bi().await.map_err(accept_error)?;
        Ok((
            QuinnSendStream { inner: send },
            QuinnRecvStream { inner: recv },
        ))
    }

    fn local_addr(&self) -> Result<SocketAddr, NetError> {
        self.endpoint.local_addr().map_err(transport_error)
    }
}

/// Sending half of the QUIC session stream.
pub struct QuinnSendStream {
    /// Private: never exposed; use [`NetSend`].
    inner: quinn::SendStream,
}

/// Receiving half of the QUIC session stream.
pub struct QuinnRecvStream {
    /// Private: never exposed; use [`NetRecv`].
    inner: quinn::RecvStream,
}

impl NetSend for QuinnSendStream {
    fn send_frame(
        &mut self,
        body: &[u8],
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<(), NetError>> + Send + '_ {
        // Framing and limit validation run eagerly at call time, so the
        // returned future borrows only `self`: over-limit bodies fail before
        // any I/O is even scheduled, and callers keep natural lifetimes for
        // `body` and `limits`.
        let framed = encode_frame(body, limits);
        async move {
            let frame = framed?;
            self.inner
                .write_all(&frame)
                .await
                .map_err(|error| match error {
                    quinn::WriteError::ClosedStream | quinn::WriteError::Stopped(_) => {
                        NetError::PeerClosed
                    }
                    other => NetError::TransportWrite {
                        detail: other.to_string(),
                    },
                })?;
            Ok(())
        }
    }

    async fn finish(&mut self) -> Result<(), NetError> {
        self.inner.finish().map_err(|_| NetError::PeerClosed)?;
        Ok(())
    }
}

impl NetRecv for QuinnRecvStream {
    fn recv_frame(
        &mut self,
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, NetError>> + Send + '_ {
        // `NetLimits` is `Copy`: snapshot it so the future borrows only `self`.
        let limits = *limits;
        async move {
            let mut header = [0u8; FRAME_HEADER_LEN];
            self.inner
                .read_exact(&mut header)
                .await
                .map_err(|error| match error {
                    quinn::ReadExactError::FinishedEarly(received) => NetError::TruncatedFrame {
                        claimed: FRAME_HEADER_LEN,
                        received,
                    },
                    quinn::ReadExactError::ReadError(read) => map_read_error(read),
                })?;
            // The gate runs on the bare prefix: allocation happens only after.
            let len = decode_frame_len(header, &limits)?;
            let mut body = vec![0u8; len];
            self.inner
                .read_exact(&mut body)
                .await
                .map_err(|error| match error {
                    quinn::ReadExactError::FinishedEarly(received) => NetError::TruncatedFrame {
                        claimed: len,
                        received,
                    },
                    quinn::ReadExactError::ReadError(read) => map_read_error(read),
                })?;
            Ok(body)
        }
    }
}

/// A reset or closed stream means the peer is gone; anything else is a
/// typed read failure (fatal to the connection — reconnecting is a new
/// session, not a resumed read).
fn map_read_error(error: quinn::ReadError) -> NetError {
    match error {
        quinn::ReadError::ClosedStream | quinn::ReadError::Reset(_) => NetError::PeerClosed,
        other => NetError::TransportRead {
            detail: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::NetEnvelope;
    use crate::ids::{NetSequence, PROTOCOL_VERSION_1};
    use std::time::Duration;

    /// Issues a self-signed certificate for `localhost` (proof/test use
    /// only — never production provisioning) and returns the DER-encoded
    /// certificate and PKCS#8 private key.
    fn localhost_identity() -> (Vec<u8>, Vec<u8>) {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("rcgen self-signed certificate");
        let certificate_der = certified.cert.der().to_vec();
        let private_key_der = certified.key_pair.serialize_der();
        (certificate_der, private_key_der)
    }

    /// Full pinning handshake plus a verified envelope exchange in both
    /// directions: the spike's core deliverable. The client trusts exactly
    /// the server's certificate, negotiates ALPN `canary-1`, and presents
    /// SNI `localhost`, which must match the certificate SAN.
    #[tokio::test(flavor = "multi_thread")]
    async fn quinn_pinning_handshake_and_envelope_round_trip() {
        let limits = NetLimits::default();
        let (certificate_der, private_key_der) = localhost_identity();

        let server = QuinnTransport::server(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &certificate_der,
            &private_key_der,
        )
        .expect("bind server");
        let server_addr = server.local_addr().expect("server address");
        let server_task = tokio::spawn(async move { server.accept().await });

        let client = QuinnTransport::client(&certificate_der).expect("bind client");
        let (mut client_send, mut client_recv) = tokio::time::timeout(
            Duration::from_secs(10),
            client.connect(server_addr, "localhost"),
        )
        .await
        .expect("connect did not hang")
        .expect("pinned handshake");

        // Client speaks first: the send below is what lets the server's
        // `accept` complete (QUIC buffers it), so it must precede the join.
        let outbound =
            NetEnvelope::seal(PROTOCOL_VERSION_1, NetSequence(1), b"client input".to_vec())
                .encode(&limits)
                .expect("encode");
        client_send
            .send_frame(&outbound, &limits)
            .await
            .expect("send");

        let (mut server_send, mut server_recv) =
            tokio::time::timeout(Duration::from_secs(10), server_task)
                .await
                .expect("accept did not hang")
                .expect("accept task")
                .expect("inbound handshake");

        // Client -> server: sealed envelope over the framed stream.
        let inbound = server_recv.recv_frame(&limits).await.expect("receive");
        let envelope = NetEnvelope::decode(&inbound).expect("decode and verify");
        assert_eq!(envelope.sequence, NetSequence(1));
        assert_eq!(envelope.payload, b"client input");

        // Server -> client: reply on the same stream pair.
        let reply = NetEnvelope::seal(
            PROTOCOL_VERSION_1,
            NetSequence(2),
            b"authoritative delta".to_vec(),
        )
        .encode(&limits)
        .expect("encode");
        server_send
            .send_frame(&reply, &limits)
            .await
            .expect("reply");
        let inbound = client_recv
            .recv_frame(&limits)
            .await
            .expect("receive reply");
        let envelope = NetEnvelope::decode(&inbound).expect("decode and verify reply");
        assert_eq!(envelope.sequence, NetSequence(2));
        assert_eq!(envelope.payload, b"authoritative delta");

        client_send.finish().await.expect("finish");
    }

    /// A client pinning the wrong certificate refuses the handshake: pinning
    /// is load-bearing, not decorative. The failure must be a transport error
    /// (TLS verification), and it must arrive promptly, not hang.
    #[tokio::test(flavor = "multi_thread")]
    async fn quinn_wrong_pin_refuses_handshake() {
        let (certificate_der, private_key_der) = localhost_identity();
        let (wrong_pin, _) = localhost_identity();

        let server = QuinnTransport::server(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &certificate_der,
            &private_key_der,
        )
        .expect("bind server");
        let server_addr = server.local_addr().expect("server address");
        let _server_task = tokio::spawn(async move { server.accept().await });

        let client = QuinnTransport::client(&wrong_pin).expect("bind client");
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            client.connect(server_addr, "localhost"),
        )
        .await;
        match outcome {
            Err(_) => panic!("wrong-pin handshake hung instead of failing"),
            Ok(Err(NetError::TransportConnect { .. })) => {}
            Ok(Err(other)) => panic!("wrong pin failed with {other:?}, expected a connect error"),
            Ok(Ok(_)) => panic!("wrong pin unexpectedly completed the handshake"),
        }
    }
}
