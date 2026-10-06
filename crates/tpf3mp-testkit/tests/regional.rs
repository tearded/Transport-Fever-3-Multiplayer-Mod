//! A release with several servers (docs/DECISIONS.md, D12's PROPOSED
//! amendment of 2026-10-06): launchers play on all of them. Rooms are hosted
//! on the closest server that answers, every server's public rooms are
//! listed together, each with its server, and an invite, a code alone,
//! joins wherever its room is; an invite naming a server the release does
//! not list is refused.

#![allow(clippy::unwrap_used)]

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tpf3mp_agent::{
    TunnelChoice, Worlds,
    launcher::{Launcher, LauncherConfig, ListedServer},
};
use tpf3mp_net::{Identity, ServerIdentity, ServerTrust};
use tpf3mp_proto::RoomSettings;
use tpf3mp_server::{Server, ServerConfig};
use tpf3mp_testkit::{scenario::toy_content, toy::toy_rules_menu};

const WAIT: Duration = Duration::from_secs(60);

/// A page's view of one launcher: where it listens and its token.
struct Page {
    address: SocketAddr,
    token: String,
}

impl Page {
    fn of(launcher: &Launcher) -> Self {
        let url = launcher.url().unwrap().strip_prefix("http://").unwrap();
        let (address, token) = url.split_once("/#").unwrap();
        Self {
            address: address.parse().unwrap(),
            token: token.to_owned(),
        }
    }

    async fn state(&self) -> Value {
        let (status, body) = self.request("GET", "/api/state", None).await;
        assert_eq!(status, 200, "{body}");
        body
    }

    /// Sends `action`; the status and the answer.
    async fn try_act(&self, action: &Value) -> (u16, Value) {
        self.request("POST", "/api/action", Some(&action.to_string()))
            .await
    }

    async fn act(&self, action: Value) {
        let (status, body) = self.try_act(&action).await;
        assert_eq!(status, 200, "{action} was refused: {body}");
    }

    async fn wait_for(&self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let found = tokio::time::timeout(WAIT, async {
            loop {
                let state = self.state().await;
                if done(&state) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        match found {
            Ok(state) => state,
            Err(_) => panic!(
                "timed out waiting for {what}; the page shows {}",
                self.state().await
            ),
        }
    }

    async fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, Value) {
        let mut stream = TcpStream::connect(self.address).await.unwrap();
        let body = body.unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nX-Launcher-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            self.token,
            body.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(body.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }
}

/// A launcher of a release that lists `servers`, the first its default.
fn launcher_config(
    root: &Path,
    name: &str,
    trust: &ServerTrust,
    servers: &[(&str, &str)],
) -> LauncherConfig {
    let servers: Vec<ListedServer> = servers
        .iter()
        .map(|(name, address)| ListedServer {
            name: (*name).to_owned(),
            address: (*address).to_owned(),
        })
        .collect();
    LauncherConfig {
        diagnostics: None,
        game_logs: None,
        hook: None,
        game_exe: None,
        game_env: Vec::new(),
        start_save: None,
        listen: "127.0.0.1:0".parse().unwrap(),
        server: Some(servers[0].address.clone()),
        server_fixed: true,
        default_server: Some(servers[0].address.clone()),
        server_name: Some(servers[0].name.clone()),
        servers,
        tunnel: TunnelChoice::Off,
        remember: None,
        trust: trust.clone(),
        identity: Arc::new(Identity::generate().unwrap().0),
        name: name.into(),
        content: toy_content(),
        mods: None,
        picker: None,
        installed: None,
        link: format!("tpf3mp-regional-{}-{name}", std::process::id()),
        worlds: Worlds::open(&root.join(name), 1 << 30).unwrap(),
        room_settings: RoomSettings {
            steps_per_second: 100,
            input_delay_ms: 60,
            checkpoint_interval: 20,
        },
    }
}

/// Starts a server on a free port with `identity`; its address and the
/// sender that stops it.
fn start_server(identity: ServerIdentity) -> (String, tokio::sync::oneshot::Sender<()>) {
    let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
    config.rules = toy_rules_menu();
    config.tick = Duration::from_millis(25);
    config.max_sessions_per_address = 100;
    config.max_handshakes_per_address = 100;
    let server = Server::bind(config).unwrap();
    let address = server.local_addr().unwrap().to_string();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));
    (address, stop)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rooms_go_to_the_closest_server_and_are_found_on_every_one() {
    let root = tempfile::tempdir().unwrap();
    // Both servers show the same certificate, which the launchers pin.
    let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
    let trust = ServerTrust::Pinned(identity.leaf().clone());
    let (eu, _stop_eu) = start_server(identity.clone());
    let (us, _stop_us) = start_server(identity);
    // A server that never answers: nothing listens there.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();

    // Ann's release lists a server that does not answer first: she plays,
    // and hosts, on the one that does.
    let ann = Launcher::start(launcher_config(
        root.path(),
        "ann",
        &trust,
        &[("EU", &silent), ("US", &us)],
    ))
    .await
    .unwrap();
    let ann_page = Page::of(&ann);
    ann_page
        .act(json!({ "action": "connect", "server": "", "name": "Ann" }))
        .await;
    let state = ann_page.state().await;
    assert_eq!(state["server"], us, "the server that answers");
    assert_eq!(state["server_name"], "US");
    ann_page
        .act(json!({
            "action": "create", "room": "transatlantic", "max_players": 4, "password": null,
            "listing": { "map": "dry", "year": 1850 },
        }))
        .await;
    let state = ann_page
        .wait_for("Ann's room", |state| state["room"]["invite"].is_string())
        .await;
    let invite = state["room"]["invite"].as_str().unwrap().to_owned();
    assert!(
        !invite.contains(' '),
        "a code alone, as with one server: {invite}"
    );
    assert_eq!(state["server"], us);
    let rows = state["servers"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{state}");
    assert_eq!(rows[0]["reachable"], false);
    assert_eq!(rows[1]["here"], true);

    // Bob plays on EU, the first of two equally close servers, and joins
    // Ann's room by its code alone: EU has no such room, US has.
    let bob = Launcher::start(launcher_config(
        root.path(),
        "bob",
        &trust,
        &[("EU", &eu), ("US", &us)],
    ))
    .await
    .unwrap();
    let bob_page = Page::of(&bob);
    bob_page
        .act(json!({ "action": "connect", "server": "", "name": "Bob" }))
        .await;
    assert_eq!(bob_page.state().await["server"], eu, "ties go to the first");
    bob_page
        .wait_for("Bob's lookout on US", |state| {
            state["servers"]
                .as_array()
                .is_some_and(|rows| rows.len() == 2 && rows[1]["reachable"] == true)
        })
        .await;
    bob_page
        .act(json!({ "action": "join", "invite": invite, "password": null }))
        .await;
    let state = bob_page
        .wait_for("Bob in Ann's room", |state| state["room"].is_object())
        .await;
    assert_eq!(state["server"], us, "joined where the room is");
    assert_eq!(state["room"]["invite"], invite.as_str());

    // Cat lists the rooms of both servers, each with its server, and joins
    // from the list. An invite naming a server the release does not list
    // is refused.
    let cat = Launcher::start(launcher_config(
        root.path(),
        "cat",
        &trust,
        &[("EU", &eu), ("US", &us)],
    ))
    .await
    .unwrap();
    let cat_page = Page::of(&cat);
    cat_page
        .act(json!({ "action": "connect", "server": "", "name": "Cat" }))
        .await;
    let (status, body) = cat_page
        .try_act(&json!({
            "action": "join", "invite": format!("evil.example.org:29470 {invite}"), "password": null,
        }))
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(body.to_string().contains("does not vouch for"), "{body}");
    let state = cat_page
        .wait_for("Cat's lookout on US", |state| {
            state["servers"]
                .as_array()
                .is_some_and(|rows| rows.len() == 2 && rows[1]["reachable"] == true)
        })
        .await;
    assert_eq!(state["server"], eu);
    cat_page
        .act(json!({ "action": "list_rooms", "page": 0 }))
        .await;
    let state = cat_page.state().await;
    let rooms = state["rooms"]["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 1, "{state}");
    assert_eq!(rooms[0]["name"], "transatlantic");
    assert_eq!(rooms[0]["server"], "US");
    assert!(rooms[0]["ping_ms"].is_u64(), "{state}");
    cat_page
        .act(json!({ "action": "join", "invite": rooms[0]["invite"], "password": null }))
        .await;
    let state = cat_page
        .wait_for("Cat in Ann's room", |state| state["room"].is_object())
        .await;
    assert_eq!(state["server"], us);
    ann_page
        .wait_for("all three in the room", |state| {
            state["room"]["members"]
                .as_array()
                .is_some_and(|members| members.len() == 3)
        })
        .await;
}
