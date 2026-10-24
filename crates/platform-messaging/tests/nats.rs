//! Real NATS JetStream tests: adapter conformance, persistence across broker restart, and
//! broker-enforced authorization. Uses the same `nats.conf` as the Compose deployment.
//!
//! Needs a `nats-server` binary (`NATS_SERVER_BIN` or on PATH); skipped when absent unless
//! `MIDGARD_REQUIRE_NATS=1` (set in CI).

use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

use platform_messaging::{
    BusError, MessageBus, conformance,
    nats::{NatsBus, NatsConfig},
};

const CONF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deployments/compose/nats/nats.conf"
);

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn binary() -> Option<String> {
    let bin = std::env::var("NATS_SERVER_BIN").unwrap_or_else(|_| "nats-server".into());
    Command::new(&bin)
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .ok()?
        .success()
        .then_some(bin)
}

#[allow(clippy::zombie_processes)] // reaped in `Server::drop`
async fn start(bin: &str, port: u16, store: &std::path::Path) -> Server {
    let child = Command::new(bin)
        .args(["-c", CONF, "-p", &port.to_string(), "-m", "0"])
        .env("NATS_STORE_DIR", store)
        .env("NATS_ADMIN_PASSWORD", "admin-pw")
        .env("NATS_PROVIDER_PASSWORD", "provider-pw")
        .env("NATS_CLIENT_PASSWORD", "client-pw")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let url = format!("127.0.0.1:{port}");
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(&url).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(300)).await; // JetStream ready
            return Server { child, port };
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("nats-server did not start");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cfg(port: u16, user: &str, pw: &str, provision: bool) -> NatsConfig {
    NatsConfig {
        url: format!("nats://127.0.0.1:{port}"),
        user: Some(user.into()),
        password: Some(pw.into()),
        stream: "EXAMPLE".into(),
        subjects: vec!["example.>".into()],
        duplicate_window: Duration::from_secs(120),
        provision,
    }
}

macro_rules! need_nats {
    () => {
        match binary() {
            Some(b) => b,
            None if std::env::var("MIDGARD_REQUIRE_NATS").is_ok() => {
                panic!("nats-server not found")
            }
            None => {
                eprintln!("skipping: nats-server not available");
                return;
            }
        }
    };
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("midgard-nats-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn nats_adapter_conforms() {
    let bin = need_nats!();
    let s = start(&bin, free_port(), &tmp("conf")).await;
    let bus = NatsBus::connect(cfg(s.port, "admin", "admin-pw", true))
        .await
        .unwrap();
    conformance::run_all(&bus, "example.conf").await;
}

#[tokio::test]
async fn messages_survive_broker_restart() {
    let bin = need_nats!();
    let store = tmp("restart");
    let port = free_port();
    let s = start(&bin, port, &store).await;
    let bus = NatsBus::connect(cfg(port, "admin", "admin-pw", true))
        .await
        .unwrap();
    bus.publish("example.persist.q", conformance::msg("p1"))
        .await
        .unwrap();
    drop(bus);
    drop(s); // broker killed

    let _s2 = start(&bin, port, &store).await;
    let bus = NatsBus::connect(cfg(port, "admin", "admin-pw", true))
        .await
        .unwrap();
    let mut c = bus
        .consumer(conformance::cfg("example", "persist", "example.persist.q"))
        .await
        .unwrap();
    let d = c
        .next(Duration::from_secs(3))
        .await
        .unwrap()
        .expect("persisted message delivered after restart");
    assert_eq!(d.message.id, "p1");
    d.ack().await.unwrap();
}

#[tokio::test]
async fn broker_enforces_publish_and_consume_permissions() {
    let bin = need_nats!();
    let s = start(&bin, free_port(), &tmp("acl")).await;
    // Provisioning identity creates the stream and the provider's durable consumer.
    let admin = NatsBus::connect(cfg(s.port, "admin", "admin-pw", true))
        .await
        .unwrap();
    let mut pc = conformance::cfg("example", "provider-submit", "example.echo.submit.*");
    pc.name = "provider-submit".into();
    pc.dead_letter = Some("example.echo.dead.submit".into());
    let _ = admin.consumer(pc.clone()).await.unwrap();

    // anonymous and wrong-password connections are refused
    assert!(
        NatsBus::connect(NatsConfig {
            user: None,
            password: None,
            ..cfg(s.port, "", "", false)
        })
        .await
        .is_err()
    );
    assert!(
        NatsBus::connect(cfg(s.port, "client-obroklik", "wrong", false))
            .await
            .is_err()
    );

    // client application: may publish to its own subject only
    let client = NatsBus::connect(cfg(s.port, "client-obroklik", "client-pw", false))
        .await
        .unwrap();
    client
        .publish("example.echo.submit.obroklik", conformance::msg("k1"))
        .await
        .expect("own subject allowed");
    let other = client
        .publish("example.echo.submit.fleet", conformance::msg("k2"))
        .await;
    assert!(
        matches!(other, Err(BusError::PublishFailed(_))),
        "other app's subject must be denied: {other:?}"
    );
    // ... and may not consume at all
    let spy = client.consumer(pc.clone()).await;
    if let Ok(mut c) = spy {
        assert!(matches!(
            c.next(Duration::from_millis(500)).await,
            Ok(None) | Err(_)
        ));
    }

    // provider: consumes only; cannot publish to a client's subject
    let provider = NatsBus::connect(cfg(s.port, "provider", "provider-pw", false))
        .await
        .unwrap();
    let mut c = provider.consumer(pc.clone()).await.unwrap();
    let d = c
        .next(Duration::from_secs(2))
        .await
        .unwrap()
        .expect("provider receives obroklik's message");
    assert_eq!(
        d.destination, "example.echo.submit.obroklik",
        "caller identity is bound to the subject"
    );
    assert_eq!(d.message.id, "k1");
    d.ack().await.unwrap();
    assert!(
        c.next(Duration::from_millis(500)).await.unwrap().is_none(),
        "denied publish stored nothing"
    );
    let forged = provider
        .publish("example.echo.submit.obroklik", conformance::msg("forged"))
        .await;
    assert!(forged.is_err(), "provider must not publish as a client");
}
