//! Snapshots at the edges: who may join a running game, who may fetch or
//! upload a world, and the world an owner hands a room to start from. The whole flow, with games that really save and load, is
//! in `tpf3mp-testkit`'s scenarios.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;

use common::{FAST, Player, RunningServer, TestClient, content, join, modded, seat};
use tpf3mp_agent::{ClientError, ClientEvent, Worlds, transfer};
use tpf3mp_net::read_message;
use tpf3mp_proto::{
    BULK_REQUEST_MAX_FRAME, BULK_RESPONSE_MAX_FRAME, BulkOpen, BulkRequest, BulkResponse,
    FixedBytes, Invite, Request, RequestError, RoomView, SavedWorld, SnapshotId, StartSave, Text,
    WorldOffer,
};
use tpf3mp_server::{ServerConfig, SnapshotConfig};

fn saving(dir: &Path) -> impl FnOnce(&mut ServerConfig) + use<> {
    let dir = dir.to_owned();
    move |config: &mut ServerConfig| {
        config.snapshots = Some(SnapshotConfig::new(dir.join("snapshots")));
    }
}

fn saving_and_logging(dir: &Path, secret: [u8; 32]) -> impl FnOnce(&mut ServerConfig) + use<> {
    let dir = dir.to_owned();
    move |config: &mut ServerConfig| {
        config.snapshots = Some(SnapshotConfig::new(dir.join("snapshots")));
        config.data_dir = Some(dir.join("rooms"));
        config.secret = secret;
    }
}

/// Seats the clients, starts the game and plays it a little, every player
/// at once: the room holds its clock until all have loaded.
async fn running(mut clients: Vec<TestClient>) -> (Vec<Player>, Invite) {
    let mut seats: Vec<&mut TestClient> = clients.iter_mut().collect();
    let invite = seat(&mut seats, FAST).await;
    clients[0].client.start_game().await.unwrap();
    let tasks: Vec<_> = clients
        .into_iter()
        .map(|client| {
            tokio::spawn(async move {
                let mut player = Player::new(client);
                player.play_until(|p| p.executed >= 1).await;
                player
            })
        })
        .collect();
    let mut players = Vec::new();
    for task in tasks {
        players.push(task.await.unwrap());
    }
    (players, invite)
}

/// Joins the running game of `invite` as a newcomer whose game runs the
/// content `content_of`, or who declared none.
async fn newcomer(
    player: &TestClient,
    invite: &Invite,
    content_of: Option<u8>,
) -> Result<tpf3mp_proto::RoomView, ClientError> {
    if let Some(value) = content_of {
        player.client.declare_content(content(value)).await?;
    }
    player.client.join_room(join(invite)).await
}

#[tokio::test]
async fn a_newcomer_must_run_the_games_content() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let (_players, invite) = running(vec![server.client("ann").await]).await;
    let mut cat = server.client("cat").await;
    for wrong in [None, Some(2)] {
        assert_eq!(
            newcomer(&cat, &invite, wrong).await.unwrap_err(),
            ClientError::Refused(RequestError::ContentMismatch),
            "content {wrong:?}"
        );
    }
    // The refusal says how the newcomer's game differs from the game's.
    let diff = cat.content_diff().await.unwrap();
    let builds = diff.game.unwrap();
    assert_eq!(
        (builds.room.as_str(), builds.yours.as_str()),
        ("build-1", "build-2")
    );
    let room = newcomer(&cat, &invite, Some(1)).await.unwrap();
    assert_eq!(room.members.len(), 2, "a seat at the running game");
    server.shut_down().await;
}

/// A newcomer whose copy of TPF3-MP's own mod has the same revision but
/// other files (its version carries their fingerprint,
/// `tpf3mp_agent::own_mod`) is refused at a running game, and told so.
#[tokio::test]
async fn a_newcomer_with_another_copy_of_tpf3mp_itself_is_refused() {
    const HOSTS: &str = "tpf3mp_1 1+0123456789abcdef";
    const OLD: &str = "tpf3mp_1 1+fedcba9876543210";
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let mut ann = server.client("ann").await;
    let invite = seat(&mut [&mut ann], FAST).await;
    ann.client
        .declare_content(modded(&["trains 1", HOSTS]))
        .await
        .unwrap();
    ann.client.start_game().await.unwrap();
    let mut player = Player::new(ann);
    player.play_until(|p| p.executed >= 1).await;

    let mut cat = server.client("cat").await;
    cat.client
        .declare_content(modded(&["trains 1", OLD]))
        .await
        .unwrap();
    assert_eq!(
        cat.client.join_room(join(&invite)).await.unwrap_err(),
        ClientError::Refused(RequestError::ContentMismatch)
    );
    assert_eq!(
        cat.content_diff().await.unwrap().to_string(),
        "Your TPF3-MP mod differs from the host's (yours fedcba98, host 01234567): reinstall the same version"
    );
    cat.client
        .declare_content(modded(&["trains 1", HOSTS]))
        .await
        .unwrap();
    let room = cat.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(room.members.len(), 2, "a seat at the running game");
    drop(player);
    server.shut_down().await;
}

#[tokio::test]
async fn a_restored_game_still_says_how_a_newcomer_differs() {
    let dir = tempfile::tempdir().unwrap();
    let secret = [4; 32];
    let server = RunningServer::start(saving_and_logging(dir.path(), secret)).await;
    let (mut players, invite) = running(vec![server.client("ann").await]).await;
    players[0].play_until(|p| p.executed >= 20).await;
    drop(players);
    server.shut_down().await;

    // The game's content survives in its log.
    let server = RunningServer::start(saving_and_logging(dir.path(), secret)).await;
    let mut cat = server.client("cat").await;
    assert_eq!(
        newcomer(&cat, &invite, Some(3)).await.unwrap_err(),
        ClientError::Refused(RequestError::ContentMismatch)
    );
    let builds = cat.content_diff().await.unwrap().game.unwrap();
    assert_eq!(
        (builds.room.as_str(), builds.yours.as_str()),
        ("build-1", "build-3")
    );
    server.shut_down().await;
}

#[tokio::test]
async fn a_kicked_player_stays_out_even_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let secret = [3; 32];
    let server = RunningServer::start(saving_and_logging(dir.path(), secret)).await;
    let (mut players, invite) = running(vec![server.client("ann").await]).await;
    // Bob joins the running game, then Ann removes him.
    let bob = server.client("bob").await;
    let bob_identity = std::sync::Arc::clone(&bob.identity);
    newcomer(&bob, &invite, Some(1)).await.unwrap();
    players[0].client().kick(bob.client.player()).await.unwrap();
    drop(bob);
    let refused = ClientError::Refused(RequestError::BadInvite);
    let bob = server
        .client_as(std::sync::Arc::clone(&bob_identity), "bob")
        .await;
    assert_eq!(newcomer(&bob, &invite, Some(1)).await.unwrap_err(), refused);
    drop(bob);
    // Ann plays on, so the kick is logged.
    players[0].play_until(|p| p.executed >= 20).await;
    drop(players);
    server.shut_down().await;

    let server = RunningServer::start(saving_and_logging(dir.path(), secret)).await;
    let bob = server.client_as(bob_identity, "bob").await;
    assert_eq!(
        newcomer(&bob, &invite, Some(1)).await.unwrap_err(),
        refused,
        "the restored room remembers the kick"
    );
    server.shut_down().await;
}

#[tokio::test]
async fn nobody_fetches_a_world_they_were_not_offered() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let (players, _invite) = running(vec![server.client("ann").await]).await;
    // A member of a running game, and someone in no room at all.
    let outsider = server.client("eve").await;
    for client in [players[0].client(), &outsider.client] {
        let (_send, mut recv) = client
            .bulk()
            .open(BulkOpen::Fetch {
                snapshot: SnapshotId(FixedBytes([0x5a; 32])),
            })
            .await
            .unwrap();
        let answer = read_message::<BulkResponse>(&mut recv, BULK_RESPONSE_MAX_FRAME)
            .await
            .unwrap();
        assert_eq!(answer, BulkResponse::Unavailable);
    }
    server.shut_down().await;
}

#[tokio::test]
async fn nobody_uploads_a_world_nobody_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let (players, _invite) = running(vec![server.client("ann").await]).await;
    let (_send, mut recv) = players[0]
        .client()
        .bulk()
        .open(BulkOpen::Serve {
            snapshot: SnapshotId(FixedBytes([0x5a; 32])),
        })
        .await
        .unwrap();
    // The server ends the stream without asking for anything.
    let request = read_message::<BulkRequest>(&mut recv, BULK_REQUEST_MAX_FRAME).await;
    assert!(
        request.as_ref().is_err_and(|error| error.is_disconnect()),
        "{request:?}"
    );
    server.shut_down().await;
}

/// A save of `len` bytes, cut into a player's store: the world the owner
/// hands over.
fn owners_save(dir: &Path, len: usize) -> (Worlds, Vec<u8>, SavedWorld) {
    let worlds = Worlds::open(&dir.join("ann"), 1 << 30).unwrap();
    let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
    let file = dir.join("start.sav");
    std::fs::write(&file, &bytes).unwrap();
    let (_, world) = worlds.ingest_copy(&file).unwrap();
    assert!(file.is_file(), "the player's own save stays");
    (worlds, bytes, world)
}

/// Another save of `len` bytes in the same player's store.
fn another_save(worlds: &Worlds, dir: &Path, name: &str, len: usize) -> SavedWorld {
    let bytes: Vec<u8> = (0..len).map(|i| (i * 13 % 241) as u8).collect();
    let file = dir.join(format!("{name}.sav"));
    std::fs::write(&file, &bytes).unwrap();
    worlds.ingest_copy(&file).unwrap().1
}

/// A save as the owner names it.
fn named(name: &str, map: &str, year: u16) -> StartSave {
    StartSave {
        name: Text::new(name).unwrap(),
        map: Text::new(map).unwrap(),
        year,
    }
}

/// The owner names `world` as the room's start, as `save`.
async fn name_start(owner: &TestClient, world: SavedWorld, save: StartSave) {
    owner
        .client
        .requests()
        .done(Request::StartWorld { world, save })
        .await
        .unwrap();
}

/// Waits until the room asks the owner for `world`, then uploads it.
async fn upload_when_asked(owner: &mut TestClient, worlds: &Worlds, world: SavedWorld) {
    let asked = owner
        .wait_for(|event| match event {
            ClientEvent::Upload { event, snapshot } => Some((event, snapshot)),
            _ => None,
        })
        .await;
    assert_eq!(asked, (0, world.snapshot));
    transfer::upload_world(&owner.client.bulk(), worlds, world.snapshot)
        .await
        .unwrap();
}

fn everyone_ready(room: &RoomView) -> bool {
    room.members.iter().all(|member| member.ready)
}

fn nobody_ready(room: &RoomView) -> bool {
    room.members.iter().all(|member| !member.ready)
}

/// Starts the room, waiting while the world it starts from is still on its
/// way: the room refuses to start before it has it.
async fn start_once_the_world_is_there(owner: &TestClient) {
    for _ in 0..200 {
        match owner.client.start_game().await {
            Ok(()) => return,
            Err(ClientError::Refused(RequestError::StartWorldPending)) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(error) => panic!("the room did not start: {error}"),
        }
    }
    panic!("the world the room starts from never arrived");
}

#[tokio::test]
async fn a_room_starts_from_the_world_its_owner_handed_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let mut ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    seat(&mut [&mut ann, &mut bob], FAST).await;
    let (worlds, bytes, world) = owners_save(dir.path(), 300_000);

    // Only the owner names it.
    assert_eq!(
        bob.client
            .requests()
            .done(Request::StartWorld {
                world,
                save: named("start", "", 0),
            })
            .await
            .unwrap_err(),
        ClientError::Refused(RequestError::NotOwner)
    );
    ann.client
        .requests()
        .done(Request::StartWorld {
            world,
            save: named("start", "", 0),
        })
        .await
        .unwrap();
    let asked = ann
        .wait_for(|event| match event {
            ClientEvent::Upload { event, snapshot } => Some((event, snapshot)),
            _ => None,
        })
        .await;
    assert_eq!(asked, (0, world.snapshot), "the room asks for it at once");
    assert_eq!(
        ann.client.start_game().await.unwrap_err(),
        ClientError::Refused(RequestError::StartWorldPending),
        "no game before the room has its world"
    );
    transfer::upload_world(&ann.client.bulk(), &worlds, world.snapshot)
        .await
        .unwrap();
    start_once_the_world_is_there(&ann).await;

    // Every member, the owner too, is handed that world to load, from the
    // game's first turn.
    for player in [&mut ann, &mut bob] {
        let start = player
            .wait_for(|event| match event {
                ClientEvent::TurnStream(start) => Some(start),
                _ => None,
            })
            .await;
        assert_eq!(
            start.world,
            Some(WorldOffer {
                snapshot: world.snapshot,
                size: world.size,
            })
        );
        assert_eq!(
            (start.next_turn, start.next_event, start.sealed_through),
            (1, 1, 0)
        );
    }
    // And can fetch it, byte for byte.
    let bobs = Worlds::open(&dir.path().join("bob"), 1 << 30).unwrap();
    let offer = WorldOffer {
        snapshot: world.snapshot,
        size: world.size,
    };
    let (file, _) = transfer::fetch_world(&bob.client.bulk(), &bobs, offer, |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(file).unwrap(), bytes);
    server.shut_down().await;
}

#[tokio::test]
async fn a_room_is_handed_no_world_where_the_server_keeps_none() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(|_| {}).await;
    let mut ann = server.client("ann").await;
    seat(&mut [&mut ann], FAST).await;
    let (_worlds, _, world) = owners_save(dir.path(), 1000);
    assert_eq!(
        ann.client
            .requests()
            .done(Request::StartWorld {
                world,
                save: named("start", "", 0),
            })
            .await
            .unwrap_err(),
        ClientError::Refused(RequestError::WorldsNotKept)
    );
    // The room starts as before, from the owner's game.
    ann.client.start_game().await.unwrap();
    server.shut_down().await;
}

#[tokio::test]
async fn the_owner_replaces_the_start_world_in_the_lobby_and_everyone_readies_again() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let mut ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    seat(&mut [&mut ann, &mut bob], FAST).await;
    let (worlds, _, first) = owners_save(dir.path(), 300_000);

    // The first named: every member sees it, on its way. It is the world
    // the room was waiting for, so who was ready stays ready.
    name_start(&ann, first, named("first", "", 0)).await;
    let room = bob
        .room_where(|room| {
            room.start
                .as_ref()
                .is_some_and(|start| start.save.name.as_str() == "first" && !start.arrived)
        })
        .await;
    assert!(everyone_ready(&room), "{room:?}");
    upload_when_asked(&mut ann, &worlds, first).await;
    bob.room_where(|room| room.start.as_ref().is_some_and(|start| start.arrived))
        .await;

    // The same save again, with what the owner's game read of it: only what
    // the room shows changes, and nobody is asked to agree again.
    name_start(&ann, first, named("first", "dry", 1900)).await;
    let room = bob
        .room_where(|room| {
            room.start
                .as_ref()
                .is_some_and(|start| start.save.map.as_str() == "dry")
        })
        .await;
    assert!(everyone_ready(&room), "{room:?}");
    assert!(room.start.as_ref().unwrap().arrived);
    assert_eq!(room.start.as_ref().unwrap().save.year, 1900);

    // Another save: asked for at once, everyone not ready again, and the
    // game waits for it.
    let second = another_save(&worlds, dir.path(), "second", 200_000);
    name_start(&ann, second, named("second", "tropical", 1950)).await;
    let room = bob
        .room_where(|room| {
            room.start
                .as_ref()
                .is_some_and(|start| start.save.name.as_str() == "second")
        })
        .await;
    assert!(!room.start.as_ref().unwrap().arrived);
    assert!(nobody_ready(&room), "they agreed to the first: {room:?}");
    for player in [&ann, &bob] {
        player.client.set_ready(true).await.unwrap();
    }
    assert_eq!(
        ann.client.start_game().await.unwrap_err(),
        ClientError::Refused(RequestError::StartWorldPending),
        "not before the room has the new save"
    );
    upload_when_asked(&mut ann, &worlds, second).await;
    start_once_the_world_is_there(&ann).await;
    for player in [&mut ann, &mut bob] {
        let start = player
            .wait_for(|event| match event {
                ClientEvent::TurnStream(start) => Some(start),
                _ => None,
            })
            .await;
        assert_eq!(
            start.world.map(|offer| offer.snapshot),
            Some(second.snapshot),
            "every game loads the save named last"
        );
    }
    server.shut_down().await;
}

#[tokio::test]
async fn the_owner_takes_the_start_world_back_and_the_owners_game_provides_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let mut ann = server.client("ann").await;
    let mut bob = server.client("bob").await;
    seat(&mut [&mut ann, &mut bob], FAST).await;
    let (worlds, _, world) = owners_save(dir.path(), 100_000);
    name_start(&ann, world, named("start", "", 0)).await;
    upload_when_asked(&mut ann, &worlds, world).await;
    let room = bob
        .room_where(|room| room.start.as_ref().is_some_and(|start| start.arrived))
        .await;
    assert!(everyone_ready(&room), "{room:?}");

    // Only the owner takes it back.
    assert_eq!(
        bob.client
            .requests()
            .done(Request::ClearStartWorld)
            .await
            .unwrap_err(),
        ClientError::Refused(RequestError::NotOwner)
    );
    ann.client
        .requests()
        .done(Request::ClearStartWorld)
        .await
        .unwrap();
    bob.room_where(|room| room.start.is_none() && nobody_ready(room))
        .await;
    // Taking back none is no change.
    ann.client
        .requests()
        .done(Request::ClearStartWorld)
        .await
        .unwrap();
    for player in [&ann, &bob] {
        player.client.set_ready(true).await.unwrap();
    }
    ann.client.start_game().await.unwrap();
    // As without one: the owner's game plays the world it has.
    let start = ann
        .wait_for(|event| match event {
            ClientEvent::TurnStream(start) => Some(start),
            _ => None,
        })
        .await;
    assert_eq!(start.world, None, "the owner's game plays its own world");
    server.shut_down().await;
}

#[tokio::test]
async fn the_start_world_cannot_change_once_the_game_runs() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let (players, _invite) = running(vec![server.client("ann").await]).await;
    let worlds = Worlds::open(&dir.path().join("ann"), 1 << 30).unwrap();
    let world = another_save(&worlds, dir.path(), "late", 1000);
    let requests = players[0].client().requests();
    assert_eq!(
        requests
            .done(Request::StartWorld {
                world,
                save: named("late", "", 0),
            })
            .await
            .unwrap_err(),
        ClientError::Refused(RequestError::GameRunning)
    );
    assert_eq!(
        requests.done(Request::ClearStartWorld).await.unwrap_err(),
        ClientError::Refused(RequestError::GameRunning)
    );
    server.shut_down().await;
}

#[tokio::test]
async fn a_public_rooms_listing_follows_its_start_save() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let ann = server.client("ann").await;
    let cat = server.client("cat").await;
    ann.client
        .create_room(tpf3mp_proto::CreateRoom {
            listing: Some(tpf3mp_proto::RoomListing {
                map: Text::new("temperate").unwrap(),
                year: 1850,
                companies: 1,
            }),
            ..common::room("open", FAST)
        })
        .await
        .unwrap();
    let (_worlds, _, world) = owners_save(dir.path(), 1000);
    name_start(&ann, world, named("start", "dry", 1920)).await;
    let page = cat.client.list_rooms(0).await.unwrap();
    assert_eq!(page.rooms[0].listing.map.as_str(), "dry");
    assert_eq!(page.rooms[0].listing.year, 1920);
    ann.client
        .requests()
        .done(Request::ClearStartWorld)
        .await
        .unwrap();
    // One page a second for a connection.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let page = cat.client.list_rooms(0).await.unwrap();
    assert_eq!(
        (
            page.rooms[0].listing.map.as_str(),
            page.rooms[0].listing.year
        ),
        ("", 0),
        "the owner's own world: unknown until the owner says"
    );
    server.shut_down().await;
}

/// A newcomer to a running game hears the game's mods before its refusal,
/// so the launcher declares those it has and joins on its second try,
/// however many mods the room runs (the 32 named by a difference were all
/// it could learn before).
#[tokio::test]
async fn a_newcomer_hears_the_games_mods_before_its_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let server = RunningServer::start(saving(dir.path())).await;
    let mut ann = server.client("ann").await;
    let invite = seat(&mut [&mut ann], FAST).await;
    let lines: Vec<String> = (0..40).map(|n| format!("mod{n:02} 1")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let declared = common::room_of(&lines);
    ann.client.declare_room(declared.clone()).await.unwrap();
    // The room's mods changed: ready again.
    ann.client.set_ready(true).await.unwrap();
    ann.client.start_game().await.unwrap();
    let mut player = Player::new(ann);
    player.play_until(|p| p.executed >= 1).await;

    let mut cat = server.client("cat").await;
    cat.client.declare_content(content(1)).await.unwrap();
    assert_eq!(
        cat.client.join_room(join(&invite)).await.unwrap_err(),
        ClientError::Refused(RequestError::ContentMismatch)
    );
    let told = cat.room_mods().await.expect("the game's mods");
    assert_eq!(*told, declared.room_mods());
    assert_eq!(told.mods.len(), 41, "all of them, TPF3-MP's own too");
    cat.client.declare_content(told.manifest()).await.unwrap();
    let room = cat.client.join_room(join(&invite)).await.unwrap();
    assert_eq!(room.members.len(), 2, "a seat at the running game");
    drop(player);
    server.shut_down().await;
}
