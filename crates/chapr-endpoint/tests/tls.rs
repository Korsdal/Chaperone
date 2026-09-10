// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The endpoint against an HTTPS coordinator (I-018 / F2).
//!
//! This test did not exist, and its absence is the whole of I-018: TLS has been
//! on by default since phase 1, no test ever drove the client over it, and the
//! client linked Mozilla's public root store only — so the default configuration
//! was one no endpoint could talk to, and setup's advice to "install this
//! certificate as trusted on the laptops" was false.
//!
//! The certificate here is generated the same way `chapr-coord setup
//! --tls-generate` generates one — a **self-signed end-entity** certificate with
//! the host, `localhost` and `127.0.0.1` as SANs. Mirroring the shipped shape is
//! the point: a test that trusted a purpose-built CA would pass while the thing
//! customers are handed still failed.

use std::net::SocketAddr;

/// A self-signed certificate for `localhost`, in the shape coord's setup writes.
///
/// Note what this establishes, because it was an open question and the answer was
/// not obvious: rustls **does** accept a self-signed *end-entity* certificate as a
/// trust anchor — `basicConstraints` CA:TRUE is not required. So coord's generated
/// certificate needs no change, and the advice to install it in a laptop's trusted
/// root store is now true rather than merely plausible.
fn self_signed_localhost() -> (String, String) {
    let sans = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    let mut params = rcgen::CertificateParams::new(sans).unwrap();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "localhost");
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    (cert.pem(), key_pair.serialize_pem())
}

/// Serve `GET /healthz` over HTTPS on loopback; returns the bound address.
async fn serve_https(cert_pem: String, key_pem: String) -> SocketAddr {
    // A workspace build compiles rustls with both `aws-lc-rs` (axum-server) and
    // `ring` (reqwest), so the process-level provider must be chosen explicitly —
    // the same reason and the same call as `chapr-coord`'s main. Err means one is
    // already installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert_pem.into_bytes(),
        key_pem.into_bytes(),
    )
    .await
    .unwrap();

    let app = axum::Router::new().route("/healthz", axum::routing::get(|| async { "ok" }));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    addr
}

/// The capability F2 restores: a coordinator with a private certificate is
/// reachable once this machine is told to trust it.
#[tokio::test]
async fn healthz_succeeds_over_https_when_the_certificate_is_trusted() {
    let (cert_pem, key_pem) = self_signed_localhost();
    let addr = serve_https(cert_pem.clone(), key_pem).await;

    let coord = chapr_endpoint::coord_client::CoordClient::with_ca_pem(
        format!("https://localhost:{}", addr.port()),
        cert_pem.into_bytes(),
    );
    coord
        .healthz()
        .await
        .expect("an HTTPS coordinator whose certificate is trusted must be reachable");
}

/// The other half, and the reason F2 was filed as an *explanation* defect: an
/// untrusted certificate must not be reported as an unreachable coordinator.
/// It is running, it answered, and this machine declined to trust it — sending
/// an administrator to restart a healthy service is the failure.
#[tokio::test]
async fn an_untrusted_certificate_says_so_rather_than_claiming_unreachable() {
    let (cert_pem, key_pem) = self_signed_localhost();
    let addr = serve_https(cert_pem, key_pem).await;

    let coord = chapr_endpoint::coord_client::CoordClient::new(format!(
        "https://localhost:{}",
        addr.port()
    ));
    let err = coord
        .healthz()
        .await
        .expect_err("an untrusted certificate must not be accepted");

    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("certificate"),
        "the refusal must name the certificate; got: {err}"
    );
    assert!(
        msg.contains("chapr_coord_ca_cert") || msg.contains("trusted root"),
        "the refusal must name a remedy, not just a cause; got: {err}"
    );
    assert!(
        !msg.contains("unreachable"),
        "a trust failure reported as unreachable sends the administrator to the wrong machine; got: {err}"
    );
}

/// A plain-HTTP coordinator that is genuinely not there stays `CoordUnreachable`.
/// The classifier must not turn every transport failure into a certificate story.
#[tokio::test]
async fn a_closed_port_is_still_reported_as_unreachable() {
    // Bind and drop, so the port is one nothing is listening on.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let coord =
        chapr_endpoint::coord_client::CoordClient::new(format!("http://127.0.0.1:{}", addr.port()));
    let err = coord.healthz().await.expect_err("nothing is listening");
    assert!(
        matches!(err, chapr_proto::ChaprError::CoordUnreachable),
        "got: {err}"
    );
}
