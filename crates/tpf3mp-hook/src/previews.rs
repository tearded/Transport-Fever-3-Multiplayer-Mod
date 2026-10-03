//! What the players' build tools show (docs/HOOKS.md, "Build previews"):
//! the player's own, which goes to the other members, and theirs, which
//! this game shows. Advisory: nothing here touches the world, and nothing
//! is ever applied.
//!
//! - **Out.** The mod's GUI hands over the action its tool's proposal
//!   would build each time the proposal changes, or `nil` once the tool
//!   shows nothing (`preview()` in [`crate::lua`]). The step gate takes it
//!   from here at most [`MIN_INTERVAL`] apart, and sends one that still
//!   shows again every [`KEEPALIVE`], so a member that missed it, or joined
//!   since, sees it.
//! - **In.** What the room relays of each other member is kept here, the
//!   latest only, until the GUI takes the changes (`previews()`). One not
//!   heard of again for [`STALE`] is gone: its player's game stopped
//!   sending, or what said it is gone was lost on the way.
//!
//! Ported from TpF2 Multiplayer's shared build previews (`mp/previews.lua`
//! in tpf2-multiplayer: five a second at most, a one-second keepalive and
//! four seconds to go stale), on the room's control stream instead of
//! files and with the room's own action schema as the geometry.

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use tpf3mp_proto::{MAX_ROOM_MEMBERS, Payload, PlayerId, action::Action};

/// Shortest time between two of the player's previews sent: five a second.
pub const MIN_INTERVAL: Duration = Duration::from_millis(200);
/// A preview that still shows is sent again this often.
pub const KEEPALIVE: Duration = Duration::from_secs(2);
/// Another member's preview not heard of again for this long is gone.
pub const STALE: Duration = Duration::from_secs(6);

/// The player's own preview, on its way out.
#[derive(Debug, Default)]
pub struct Out {
    /// What the tool shows now; `None` when it shows nothing.
    shown: Option<Payload>,
    /// Whether `shown` changed since it was last sent.
    changed: bool,
    /// When a preview was last sent.
    sent: Option<Instant>,
}

impl Out {
    /// The tool shows `preview` now (`None`: nothing).
    pub fn set(&mut self, preview: Option<Payload>) {
        if preview != self.shown {
            self.shown = preview;
            self.changed = true;
        }
    }

    /// What to send now, if anything: the change, once [`MIN_INTERVAL`]
    /// passed since the last one sent, or what still shows, once
    /// [`KEEPALIVE`] did.
    pub fn take(&mut self, now: Instant) -> Option<Option<Payload>> {
        let since = self.sent.map(|sent| now.saturating_duration_since(sent));
        let due = if self.changed {
            since.is_none_or(|since| since >= MIN_INTERVAL)
        } else {
            self.shown.is_some() && since.is_some_and(|since| since >= KEEPALIVE)
        };
        if !due {
            return None;
        }
        self.changed = false;
        self.sent = Some(now);
        Some(self.shown.clone())
    }

    /// The room's game is over: nothing shows, and nothing is owed.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The world's GUI is gone, and the tool that showed a preview with it:
    /// the others are told it shows nothing, rather than kept seeing it.
    /// Where that hide is sent while the agent takes no previews (the world
    /// loading), it is lost, and the others drop the preview once it is no
    /// longer kept alive, six seconds on.
    pub fn world_gone(&mut self) {
        self.set(None);
    }
}

/// Another member's preview, as last heard.
#[derive(Debug)]
struct Heard {
    preview: Option<Payload>,
    at: Instant,
    /// Whether the GUI has it.
    taken: bool,
}

/// The other members' previews, as the room relays them.
#[derive(Debug, Default)]
pub struct In {
    members: BTreeMap<PlayerId, Heard>,
}

/// One change for the GUI: a member's preview, or `None` once it is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub from: PlayerId,
    pub action: Option<Action>,
}

impl In {
    /// The room relayed `preview` from `from` at `now`.
    pub fn heard(&mut self, from: PlayerId, preview: Option<Payload>, now: Instant) {
        if let Some(heard) = self.members.get_mut(&from) {
            if heard.preview != preview {
                heard.preview = preview;
                heard.taken = false;
            }
            heard.at = now;
            return;
        }
        // A member never shown: nothing to take away.
        if preview.is_none() || self.members.len() >= usize::from(MAX_ROOM_MEMBERS) {
            return;
        }
        self.members.insert(
            from,
            Heard {
                preview,
                at: now,
                taken: false,
            },
        );
    }

    /// The changes since the last call: each member's new preview, decoded,
    /// and `None` for each one gone (taken away, gone stale, or one that
    /// does not decode, which this game cannot show).
    pub fn take(&mut self, now: Instant) -> Vec<Change> {
        for heard in self.members.values_mut() {
            if heard.preview.is_some() && now.saturating_duration_since(heard.at) >= STALE {
                heard.preview = None;
                heard.taken = false;
            }
        }
        let changes: Vec<Change> = self
            .members
            .iter_mut()
            .filter(|(_, heard)| !heard.taken)
            .map(|(from, heard)| {
                heard.taken = true;
                let action = heard
                    .preview
                    .as_ref()
                    .and_then(|payload| Action::from_payload(payload).ok());
                Change {
                    from: *from,
                    action,
                }
            })
            .collect();
        self.members.retain(|_, heard| heard.preview.is_some());
        changes
    }

    /// The world's GUI is gone, and what it drew with it: every preview
    /// still shown is given to the next GUI again, as a change.
    pub fn world_gone(&mut self) {
        for heard in self.members.values_mut() {
            heard.taken = false;
        }
    }

    /// The room's game is over: every preview with it, each one shown told
    /// as gone, so the GUI clears what it drew.
    pub fn clear(&mut self) {
        self.members
            .retain(|_, heard| heard.preview.is_some() || !heard.taken);
        for heard in self.members.values_mut() {
            heard.preview = None;
            heard.taken = false;
        }
    }
}

#[derive(Debug, Default)]
struct Previews {
    out: Out,
    r#in: In,
}

static PREVIEWS: Mutex<Previews> = Mutex::new(Previews {
    out: Out {
        shown: None,
        changed: false,
        sent: None,
    },
    r#in: In {
        members: BTreeMap::new(),
    },
});

fn previews() -> MutexGuard<'static, Previews> {
    PREVIEWS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The player's tool shows `preview` now (`None`: nothing).
pub fn show(preview: Option<Payload>) {
    previews().out.set(preview);
}

/// The street/track builder reset its proposal. An inactive network tool
/// must not withdraw a construction tool's current preview.
pub fn withdraw_network() {
    let mut previews = previews();
    let network = previews
        .out
        .shown
        .as_ref()
        .and_then(|payload| Action::from_payload(payload).ok())
        .is_some_and(|action| matches!(action, Action::BuildRoad(_) | Action::BuildTrack(_)));
    if network {
        previews.out.set(None);
    }
}

/// The player's preview to send now, if any ([`Out::take`]).
pub fn take_out(now: Instant) -> Option<Option<Payload>> {
    previews().out.take(now)
}

/// The room relayed another member's preview.
pub fn heard(from: PlayerId, preview: Option<Payload>, now: Instant) {
    previews().r#in.heard(from, preview, now);
}

/// The other members' previews that changed, for the GUI ([`In::take`]).
pub fn take_in(now: Instant) -> Vec<Change> {
    previews().r#in.take(now)
}

/// The world's GUI is gone (a reload of the room's world): the player's
/// preview is withdrawn and the others' are drawn again in the next one.
pub fn world_gone() {
    let mut previews = previews();
    previews.out.world_gone();
    previews.r#in.world_gone();
}

/// The room's game is over: no preview either way.
pub fn clear() {
    let mut previews = previews();
    previews.out.clear();
    previews.r#in.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpf3mp_proto::{
        FixedBytes, Text,
        action::{Action, Bulldoze, ConstructionRef, Pos},
    };

    fn payload(n: i32) -> Payload {
        Action::Bulldoze(Bulldoze::Construction(ConstructionRef {
            file: Text::new("station/rail/modular_station.con").unwrap(),
            at: Pos { x: n, y: 0, z: 0 },
        }))
        .to_payload()
        .unwrap()
    }

    fn player(n: u8) -> PlayerId {
        PlayerId(FixedBytes([n; 32]))
    }

    #[test]
    fn a_change_goes_out_at_most_five_times_a_second() {
        let t0 = Instant::now();
        let mut out = Out::default();
        assert_eq!(out.take(t0), None, "nothing shown, nothing sent");
        out.set(Some(payload(1)));
        assert_eq!(out.take(t0), Some(Some(payload(1))));
        out.set(Some(payload(2)));
        assert_eq!(out.take(t0 + Duration::from_millis(100)), None, "too soon");
        out.set(Some(payload(3)));
        assert_eq!(
            out.take(t0 + MIN_INTERVAL),
            Some(Some(payload(3))),
            "the latest, not each one"
        );
        assert_eq!(out.take(t0 + MIN_INTERVAL), None, "once");
    }

    #[test]
    fn what_still_shows_goes_out_again_and_nothing_does_not() {
        let t0 = Instant::now();
        let mut out = Out::default();
        out.set(Some(payload(1)));
        out.take(t0);
        out.set(Some(payload(1)));
        assert_eq!(out.take(t0 + MIN_INTERVAL), None, "the same is no change");
        assert_eq!(out.take(t0 + KEEPALIVE), Some(Some(payload(1))));
        out.set(None);
        assert_eq!(out.take(t0 + KEEPALIVE + MIN_INTERVAL), Some(None));
        assert_eq!(out.take(t0 + KEEPALIVE * 5), None, "nothing to keep alive");
    }

    #[test]
    fn a_clear_owes_nothing() {
        let t0 = Instant::now();
        let mut out = Out::default();
        out.set(Some(payload(1)));
        out.clear();
        assert_eq!(out.take(t0), None);
    }

    #[test]
    fn the_gui_gets_each_members_latest_once() {
        let t0 = Instant::now();
        let mut heard = In::default();
        heard.heard(player(1), Some(payload(1)), t0);
        heard.heard(player(1), Some(payload(2)), t0);
        heard.heard(player(2), Some(payload(5)), t0);
        let changes = heard.take(t0);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].from, player(1));
        assert_eq!(
            changes[0].action,
            Some(Action::from_payload(&payload(2)).unwrap())
        );
        assert!(heard.take(t0).is_empty(), "nothing new");
        heard.heard(player(1), Some(payload(2)), t0 + KEEPALIVE);
        assert!(
            heard.take(t0 + KEEPALIVE).is_empty(),
            "a keepalive is no change"
        );
    }

    #[test]
    fn a_preview_taken_away_or_unheard_is_gone() {
        let t0 = Instant::now();
        let mut heard = In::default();
        heard.heard(player(1), Some(payload(1)), t0);
        heard.heard(player(2), Some(payload(2)), t0);
        heard.take(t0);
        heard.heard(player(1), None, t0 + KEEPALIVE);
        heard.heard(player(2), Some(payload(2)), t0 + KEEPALIVE);
        assert_eq!(
            heard.take(t0 + KEEPALIVE),
            vec![Change {
                from: player(1),
                action: None
            }]
        );
        assert!(heard.take(t0 + KEEPALIVE + STALE / 2).is_empty());
        assert_eq!(
            heard.take(t0 + KEEPALIVE + STALE),
            vec![Change {
                from: player(2),
                action: None
            }],
            "not heard of for too long"
        );
        assert!(heard.take(t0 + STALE * 3).is_empty(), "gone once");
        heard.heard(player(3), None, t0);
        assert!(
            heard.take(t0).is_empty(),
            "never shown, nothing to take away"
        );
    }

    #[test]
    fn a_reloaded_world_withdraws_the_players_preview_and_redraws_the_others() {
        let t0 = Instant::now();
        let mut out = Out::default();
        out.set(Some(payload(1)));
        assert_eq!(out.take(t0), Some(Some(payload(1))));
        out.world_gone();
        assert_eq!(
            out.take(t0 + MIN_INTERVAL),
            Some(None),
            "the tool went with the world: the others are told"
        );
        assert_eq!(out.take(t0 + KEEPALIVE * 3), None, "and nothing kept alive");

        let mut r#in = In::default();
        r#in.heard(player(1), Some(payload(1)), t0);
        assert_eq!(r#in.take(t0).len(), 1);
        r#in.world_gone();
        // The member's keepalive, unchanged, after a reload shorter than
        // STALE: the new GUI still gets it to draw.
        r#in.heard(player(1), Some(payload(1)), t0 + KEEPALIVE);
        assert_eq!(
            r#in.take(t0 + KEEPALIVE),
            [Change {
                from: player(1),
                action: Some(Action::from_payload(&payload(1)).unwrap())
            }]
        );
    }

    #[test]
    fn the_rooms_end_tells_the_gui_every_preview_is_gone() {
        let t0 = Instant::now();
        let mut r#in = In::default();
        r#in.heard(player(1), Some(payload(1)), t0);
        r#in.heard(player(2), Some(payload(2)), t0);
        assert_eq!(r#in.take(t0).len(), 2);
        r#in.clear();
        let gone = r#in.take(t0);
        assert_eq!(
            gone,
            [
                Change {
                    from: player(1),
                    action: None
                },
                Change {
                    from: player(2),
                    action: None
                }
            ],
            "each one drawn is cleared"
        );
        assert!(r#in.take(t0).is_empty(), "and told once");
        assert!(r#in.members.is_empty());
    }

    #[test]
    fn a_preview_that_does_not_decode_shows_nothing() {
        let t0 = Instant::now();
        let mut heard = In::default();
        heard.heard(player(1), Some(Payload::new(vec![0xff; 8]).unwrap()), t0);
        assert_eq!(
            heard.take(t0),
            vec![Change {
                from: player(1),
                action: None
            }]
        );
    }
}
