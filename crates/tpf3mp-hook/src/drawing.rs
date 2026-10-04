//! The other players' build previews, drawn in this game (docs/HOOKS.md,
//! "Build previews"; investigation/TPF3_BUILD_PREVIEWS_2026-10-02.md).
//!
//! The game draws a build tool's preview with a `UI::BuilderRenderer`,
//! registered with the main `CRendererComponent` ("mainView") and filled by
//! `builder_renderer_util::AddToRenderer` from the tool's proposal and the
//! `ProposalData` the game evaluated for it. The hook keeps a renderer of
//! its own for each other member whose preview shows, made by the game's
//! own `RendererFactory`, and fills it the same way:
//!
//! - **Drawing.** The Multiplayer plugin makes a member's preview into the
//!   proposal it would build here (`tpf3mp/apply.lua`), arms this thread
//!   for that member (`tpf3mp_native.draw(from)`, [`arm`]) and has the game
//!   evaluate it with `api.engine.util.proposal.makeProposalData`. That
//!   binding converts the proposal and calls `CreateProposalData`; the hook
//!   redirects that one call ([`CALL_TARGET`]), lets it run, and on the armed
//!   thread alone fills the member's renderer from the toolkit, the
//!   converted proposal and the `ProposalData` it made ([`draw_made`]). Then
//!   the plugin disarms and reads what happened (`drawn()`, [`disarm`]).
//!   Every other call of the binding, the mod's own game script's on the
//!   simulation threads included, is the game's alone.
//! - **Hiding.** A member's tool showing nothing clears their renderer
//!   (`tpf3mp_native.undraw(from)`, [`hide`]); it stays registered for the
//!   next preview.
//! - **Teardown.** `UI::CGameUI::~CGameUI` is detoured: before the game's
//!   own, the hook's renderers of that `CGameUI` are cleared, leave its
//!   main component and are destroyed ([`teardown`]).
//!
//! The renderer's tint is this game's verdict on the proposal, blue, or red
//! where this game finds errors in it or calls it critical; a critical one
//! is drawn too, as the game's own builders draw theirs.
//!
//! **Terrain.** A preview's cuts and embankments are terrain heights its
//! renderer uploads into the one view terrain every renderer shares
//! (`EndHeightMod`, `ViewTerrain::ApplyBlocks`). A renderer's `Clear`
//! resets that view terrain whole, every renderer's heights at once
//! (`0x398600` walks all of its changed blocks), so the hook composes it as
//! TpF2 Multiplayer did: after every reset, whoever's `Clear` it was, each
//! renderer's heights are applied again, the members' first and this
//! player's own tools last, so where both change the same ground this
//! player's tool shows ([`compose`]). The game's own renderers that upload
//! heights are noted in `EndHeightMod` and forgotten in their destructor.
//! Without every part of that (a build where one is missing), the hook's
//! renderers upload no heights at all (their flag at `+0xf0` cleared): a
//! preview then shows no cut or embankment, and is never left behind.
//!
//! Every target is in the profile, each offset read from the instruction
//! of the game's own that uses it ([`Layout`]); a build missing any draws
//! nothing (fail closed). Everything native runs on the GUI thread, where
//! the game's tools fill theirs.
//!
//! Ported from TpF2 Multiplayer's `native/src/preview_plugin.cpp` (build
//! 35924): one renderer per peer, filled by the game's own conversion, never
//! a command sent.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::{
    cell::{Cell, RefCell},
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::ThreadId,
};

use tpf3mp_proto::PlayerId;

pub use crate::build_data::native::drawing::ADD_TARGET;
pub use crate::build_data::native::drawing::APPLY_TARGET;
pub use crate::build_data::native::drawing::CALL_TARGET;
pub use crate::build_data::native::drawing::CLEAR_TARGET;
pub use crate::build_data::native::drawing::CREATE_TARGET;
pub use crate::build_data::native::drawing::DESTROY_TARGET;
pub use crate::build_data::native::drawing::END_HEIGHTS_TARGET;
pub use crate::build_data::native::drawing::EVALUATE_TARGET;
pub use crate::build_data::native::drawing::FACTORY_FIELD;
pub use crate::build_data::native::drawing::FILL_TARGET;
pub use crate::build_data::native::drawing::GAME_UI_DTOR_TARGET;
pub use crate::build_data::native::drawing::GAME_UI_FIELD;
pub use crate::build_data::native::drawing::MAIN_VIEW_FIELD;
pub use crate::build_data::native::drawing::MODEL_DATA_FIELD;
pub use crate::build_data::native::drawing::REMOVE_TARGET;
pub use crate::build_data::native::drawing::UPLOAD_FIELD;

/// Most members drawn at once.
pub const MAX_DRAWN: usize = 16;
/// Most of the game's own renderers whose heights are composed.
pub const MAX_LOCALS: usize = 64;

/// The offset an instruction `opcode disp32` at `code` uses, when it is
/// that instruction and the offset is a plausible field's.
pub fn field_after(code: &[u8], opcode: &[u8]) -> Option<usize> {
    if code.get(..opcode.len())? != opcode {
        return None;
    }
    let at = opcode.len();
    let disp = i32::from_le_bytes(code.get(at..at + 4)?.try_into().ok()?);
    let offset = usize::try_from(disp).ok()?;
    (offset > 0 && offset < 0x1_0000).then_some(offset)
}

/// The fields the hook reads, each from the game's own instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// `CMenuUI` -> its `CGameUI*`.
    pub game_ui: usize,
    /// `CGameUI` -> its `RendererFactory*`.
    pub factory: usize,
    /// `CGameUI` -> its main `CRendererComponent*`.
    pub main_view: usize,
    /// `CGameUI` -> its `ModelData*`.
    pub model_data: usize,
    /// `BuilderRenderer` -> its terrain upload flag (a byte).
    pub upload: usize,
}

impl Layout {
    /// The layout from each anchor's code, or the anchor that does not read.
    pub fn read(code: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Self, String> {
        let field = |(name, opcode): (&str, &[u8])| -> Result<usize, String> {
            let bytes = code(name).ok_or_else(|| format!("the profile has no {name}"))?;
            field_after(&bytes, opcode)
                .ok_or_else(|| format!("{name} is not the instruction it names"))
        };
        Ok(Self {
            game_ui: field(GAME_UI_FIELD)?,
            factory: field(FACTORY_FIELD)?,
            main_view: field(MAIN_VIEW_FIELD)?,
            model_data: field(MODEL_DATA_FIELD)?,
            upload: field(UPLOAD_FIELD)?,
        })
    }
}

pub use crate::build_data::native::drawing::UPLOAD_LEN;

/// Where `EndHeightMod` uploads a renderer's terrain heights, each offset
/// from its own instruction (build 40408, 0x7bbb6a): `cmp byte
/// [rcx+upload],0; je; mov rdx,[rcx+state]; add rdx,list; movzx r8d,byte
/// [rcx+flag]; mov rcx,[rcx+terrain]; call ApplyBlocks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upload {
    /// `BuilderRenderer` -> its upload flag (a byte).
    pub upload: usize,
    /// `BuilderRenderer` -> its state.
    pub state: usize,
    /// State -> its list of height blocks (a `std::vector`).
    pub list: usize,
    /// `BuilderRenderer` -> the flag `ApplyBlocks` takes (a byte).
    pub flag: usize,
    /// `BuilderRenderer` -> the view terrain it uploads into.
    pub terrain: usize,
    /// The address the call calls.
    pub apply: usize,
}

impl Upload {
    /// The upload from the anchor's code at `at`, or `None` when it is not
    /// that sequence.
    pub fn read(code: &[u8], at: usize) -> Option<Self> {
        let code = code.get(..UPLOAD_LEN)?;
        if code[6] != 0 || code[7] != 0x74 || code[31..34] != [0x48, 0x8B, 0x49] || code[35] != 0xE8
        {
            return None;
        }
        let terrain = usize::from(code[34]);
        if terrain == 0 || terrain >= 0x80 {
            return None;
        }
        let rel = i32::from_le_bytes(code[36..40].try_into().ok()?);
        Some(Self {
            upload: field_after(code, &[0x80, 0xB9])?,
            state: field_after(&code[9..], &[0x48, 0x8B, 0x91])?,
            list: field_after(&code[16..], &[0x48, 0x81, 0xC2])?,
            flag: field_after(&code[23..], &[0x44, 0x0F, 0xB6, 0x81])?,
            terrain,
            apply: (at + UPLOAD_LEN).wrapping_add_signed(rel as isize),
        })
    }
}

/// Renderers' terrain heights: the game's in the game, a recording
/// stand-in in the tests.
pub trait Heights {
    /// `renderer`'s view terrain, list of height blocks and flag, when it
    /// uploads heights and has any.
    fn blocks(&self, renderer: usize) -> Option<(usize, usize, u8)>;
    /// Applies a list of height blocks to a view terrain.
    fn apply(&self, terrain: usize, list: usize, flag: u8);
}

/// Applies every renderer's heights again, after the view terrain they
/// share was reset: the members' (`ours`) first, then this player's own
/// tools (`locals`), so where both change the same ground the player's own
/// tool shows. Returns how many lists were applied.
pub fn compose(heights: &dyn Heights, ours: &[usize], locals: &[usize]) -> usize {
    let mut applied = 0;
    for &renderer in ours.iter().chain(locals) {
        if let Some((terrain, list, flag)) = heights.blocks(renderer) {
            heights.apply(terrain, list, flag);
            applied += 1;
        }
    }
    applied
}

/// What the hook does with renderers: the game's functions in the game,
/// a recording stand-in in the tests.
pub trait Renderers {
    /// A new renderer from the factory, or 0.
    fn create(&self, factory: usize) -> usize;
    /// Registers `renderer` with the component `view`.
    fn register(&self, view: usize, renderer: usize);
    /// Takes `renderer` off the component `view`.
    fn unregister(&self, view: usize, renderer: usize);
    /// Clears what `renderer` shows, and resets the view terrain when the
    /// hook composes it.
    fn clear(&self, renderer: usize);
    /// Destroys `renderer`.
    fn destroy(&self, renderer: usize);
}

/// A member's renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawn {
    pub from: PlayerId,
    pub renderer: usize,
    /// The `CGameUI` it belongs to.
    pub game_ui: usize,
    /// Whether it shows a preview now.
    pub shown: bool,
}

/// The members' renderers.
#[derive(Debug, Default)]
pub struct Table {
    pub drawn: Vec<Drawn>,
}

impl Table {
    /// `from`'s renderer in `game_ui`, made and registered with `view` if
    /// it has none yet. Renderers of another `CGameUI` are forgotten, never
    /// touched: theirs is gone.
    pub fn renderer_for(
        &mut self,
        ops: &dyn Renderers,
        from: PlayerId,
        game_ui: usize,
        factory: usize,
        view: usize,
    ) -> Result<usize, String> {
        self.drawn.retain(|d| d.game_ui == game_ui);
        if let Some(d) = self.drawn.iter().find(|d| d.from == from) {
            return Ok(d.renderer);
        }
        if self.drawn.len() >= MAX_DRAWN {
            // A renderer that shows nothing now (its member hid it, or left)
            // is cleared already: it draws for this member instead.
            if let Some(d) = self.drawn.iter_mut().find(|d| !d.shown) {
                d.from = from;
                return Ok(d.renderer);
            }
            return Err(format!("{MAX_DRAWN} members' previews are drawn already"));
        }
        if factory == 0 || view == 0 {
            return Err("the world's GUI has no renderer factory or main view yet".into());
        }
        let renderer = ops.create(factory);
        if renderer == 0 {
            return Err("the game made no renderer".into());
        }
        ops.register(view, renderer);
        self.drawn.push(Drawn {
            from,
            renderer,
            game_ui,
            shown: false,
        });
        Ok(renderer)
    }

    /// Marks `from`'s renderer as showing a preview, or not.
    pub fn mark(&mut self, from: PlayerId, shown: bool) {
        if let Some(d) = self.drawn.iter_mut().find(|d| d.from == from) {
            d.shown = shown;
        }
    }

    /// Clears `from`'s renderer in `game_ui`, if it shows anything.
    pub fn hide(&mut self, ops: &dyn Renderers, from: PlayerId, game_ui: usize) {
        if let Some(d) = self
            .drawn
            .iter_mut()
            .find(|d| d.from == from && d.game_ui == game_ui && d.shown)
        {
            ops.clear(d.renderer);
            d.shown = false;
        }
    }

    /// `game_ui` is being destroyed: its renderers are cleared, leave its
    /// main component `view` and are destroyed. Returns how many.
    pub fn teardown(&mut self, ops: &dyn Renderers, game_ui: usize, view: usize) -> usize {
        let (gone, kept): (Vec<Drawn>, Vec<Drawn>) =
            self.drawn.drain(..).partition(|d| d.game_ui == game_ui);
        self.drawn = kept;
        for d in &gone {
            ops.clear(d.renderer);
            if view != 0 {
                ops.unregister(view, d.renderer);
            }
            ops.destroy(d.renderer);
        }
        gone.len()
    }
}

static TABLE: Mutex<Table> = Mutex::new(Table { drawn: Vec::new() });

fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The hook's renderers, and the game's own that upload heights, for the
/// detours. Never locked across a call into the game, and TABLE is never
/// taken under them: the game calls the detours from inside the very calls
/// the table makes.
static OURS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static LOCALS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Every part of the composition is in.
static COMPOSING: AtomicBool = AtomicBool::new(false);
/// A `CGameUI` is being destroyed: nothing is composed.
static TEARING: AtomicBool = AtomicBool::new(false);

fn ours() -> MutexGuard<'static, Vec<usize>> {
    OURS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn locals() -> MutexGuard<'static, Vec<usize>> {
    LOCALS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The hook's renderers as the table has them, for the detours.
fn note_ours(table: &Table) {
    *ours() = table.drawn.iter().map(|d| d.renderer).collect();
}

/// The menu whose frames run, and the thread they run on: the GUI's.
static MENU: AtomicUsize = AtomicUsize::new(0);
static GUI_THREAD: Mutex<Option<ThreadId>> = Mutex::new(None);

/// A frame of the game's menu (`UI::CMenuUI::DoStep`), on the GUI thread.
pub fn note_menu(menu: usize) {
    MENU.store(menu, Ordering::Release);
    let mut gui = GUI_THREAD.lock().unwrap_or_else(PoisonError::into_inner);
    if gui.is_none() {
        *gui = Some(std::thread::current().id());
    }
}

fn on_gui_thread() -> bool {
    *GUI_THREAD.lock().unwrap_or_else(PoisonError::into_inner) == Some(std::thread::current().id())
}

thread_local! {
    /// The member the next `CreateProposalData` of `makeProposalData` on
    /// this thread draws for.
    static ARMED: Cell<Option<PlayerId>> = const { Cell::new(None) };
    /// What the armed call came to, until disarmed.
    static OUTCOME: RefCell<Option<Result<(), String>>> = const { RefCell::new(None) };
}

/// The game's functions this module calls, and the layout it reads.
struct Game {
    layout: Layout,
    create: unsafe extern "C-unwind" fn(usize) -> usize,
    add: unsafe extern "C-unwind" fn(usize, usize),
    remove: unsafe extern "C-unwind" fn(usize, usize),
    clear: unsafe extern "C-unwind" fn(usize, u8, u8, u8),
    destroy: unsafe extern "C-unwind" fn(usize, u32) -> usize,
    #[allow(clippy::type_complexity)]
    fill: unsafe extern "C-unwind" fn(usize, usize, usize, usize, usize, usize, usize, u8, u8, u8),
    evaluate: EvaluateFn,
    /// The composition's: the upload's layout and `ApplyBlocks`, when every
    /// part of it is in.
    heights: Option<(Upload, ApplyFn)>,
}

/// `ViewTerrain::ApplyBlocks(terrain, list, flag)`.
type ApplyFn = unsafe extern "C-unwind" fn(usize, usize, u8) -> usize;

/// `CreateProposalData(out, toolkit, r8, proposal, preprocess, opt,
/// context)`, which returns `out`.
type EvaluateFn =
    unsafe extern "C-unwind" fn(usize, usize, usize, usize, usize, usize, usize) -> usize;

static GAME: OnceLock<Game> = OnceLock::new();
/// `~CGameUI`'s trampoline.
static GAME_UI_DTOR_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

impl Renderers for Game {
    fn create(&self, factory: usize) -> usize {
        // SAFETY: the factory the live CGameUI owns, on the GUI thread.
        unsafe { (self.create)(factory) }
    }
    fn register(&self, view: usize, renderer: usize) {
        // SAFETY: the live main component, a renderer it does not hold yet
        // (the table registers each once).
        unsafe { (self.add)(view, renderer) }
    }
    fn unregister(&self, view: usize, renderer: usize) {
        // SAFETY: as register, a renderer it holds.
        unsafe { (self.remove)(view, renderer) }
    }
    fn clear(&self, renderer: usize) {
        // Composing, as the game's own ProposalViewer clears: the view
        // terrain reset too, and composed again by the caller. Otherwise the
        // renderer never uploaded any heights, and the terrain is left alone.
        let terrain = u8::from(self.heights.is_some());
        // SAFETY: a renderer the hook made and has not destroyed, through
        // Clear's trampoline when it is detoured.
        unsafe { (self.clear)(renderer, 1, terrain, terrain) };
    }
    fn destroy(&self, renderer: usize) {
        // SAFETY: as clear, taken off its component first; slot 0 with
        // flag 1 frees it.
        unsafe {
            (self.destroy)(renderer, 1);
        }
    }
}

impl Heights for (Upload, ApplyFn) {
    fn blocks(&self, renderer: usize) -> Option<(usize, usize, u8)> {
        let upload = &self.0;
        // SAFETY: a live renderer (the hook's, or one of the game's noted in
        // EndHeightMod and not yet destroyed), read at the offsets the
        // game's own upload reads; its list is a std::vector (begin, end).
        unsafe {
            if std::ptr::read_volatile((renderer + upload.upload) as *const u8) == 0 {
                return None;
            }
            let state = pointer_at(renderer, upload.state);
            let terrain = pointer_at(renderer, upload.terrain);
            if state == 0 || terrain == 0 {
                return None;
            }
            let list = state + upload.list;
            if pointer_at(list, 0) == pointer_at(list, 8) {
                return None;
            }
            let flag = std::ptr::read_volatile((renderer + upload.flag) as *const u8);
            Some((terrain, list, flag))
        }
    }
    fn apply(&self, terrain: usize, list: usize, flag: u8) {
        // SAFETY: as EndHeightMod calls it, with a renderer's own view
        // terrain, list and flag, on the GUI thread.
        unsafe { (self.1)(terrain, list, flag) };
    }
}

/// Composes the view terrain again, after a reset: on the GUI thread, with
/// every part in, and never while a `CGameUI` is destroyed.
fn recompose() {
    let Some(heights) = GAME.get().and_then(|game| game.heights.as_ref()) else {
        return;
    };
    if !COMPOSING.load(Ordering::Acquire) || TEARING.load(Ordering::Acquire) || !on_gui_thread() {
        return;
    }
    let ours = ours().clone();
    let locals = locals().clone();
    compose(heights, &ours, &locals);
}

/// The trampolines of the detoured renderer functions.
static CLEAR_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static END_HEIGHTS_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static DESTROY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

type ClearFn = unsafe extern "C-unwind" fn(usize, u8, u8, u8) -> usize;
type EndHeightsFn = unsafe extern "C-unwind" fn(usize, usize, usize, usize) -> usize;
type DestroyFn = unsafe extern "C-unwind" fn(usize, usize) -> usize;

/// Any renderer's `Clear`, detoured: one that reset the view terrain has
/// every renderer's heights applied again after it.
unsafe extern "C-unwind" fn clear_detour(
    renderer: usize,
    models: u8,
    reset: u8,
    mods: u8,
) -> usize {
    let original = CLEAR_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: Clear's trampoline, called as the game called Clear.
    let done =
        unsafe { std::mem::transmute::<usize, ClearFn>(original)(renderer, models, reset, mods) };
    if reset != 0 {
        recompose();
    }
    done
}

/// Any renderer's `EndHeightMod`, detoured: a renderer of the game's own
/// that uploads heights is noted, to be composed after the next reset.
unsafe extern "C-unwind" fn end_heights_detour(
    renderer: usize,
    a: usize,
    b: usize,
    c: usize,
) -> usize {
    let original = END_HEIGHTS_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: EndHeightMod's trampoline, called as the game called it.
    let done = unsafe { std::mem::transmute::<usize, EndHeightsFn>(original)(renderer, a, b, c) };
    let uploads = GAME
        .get()
        .and_then(|game| game.heights.as_ref())
        .is_some_and(|heights| heights.blocks(renderer).is_some());
    if uploads && !ours().contains(&renderer) {
        let mut locals = locals();
        if !locals.contains(&renderer) && locals.len() < MAX_LOCALS {
            locals.push(renderer);
        }
    }
    done
}

/// Any renderer's destructor, detoured: it is forgotten first.
unsafe extern "C-unwind" fn destroy_detour(renderer: usize, flags: usize) -> usize {
    let mut known = false;
    locals().retain(|&r| {
        known |= r == renderer;
        r != renderer
    });
    ours().retain(|&r| {
        known |= r == renderer;
        r != renderer
    });
    // Composing reads these renderers on the GUI thread, from a copy of the
    // lists: one destroyed on another thread could be read as it goes. Not
    // seen on build 40408; said once if it ever happens.
    if known && !on_gui_thread() {
        static SAID: AtomicBool = AtomicBool::new(false);
        if !SAID.swap(true, Ordering::AcqRel) {
            crate::lua::log(
                "build previews: a renderer whose terrain is composed was destroyed off the GUI thread"
                    .to_string(),
            );
        }
    }
    let original = DESTROY_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the destructor's trampoline, called as the game called it.
    unsafe { std::mem::transmute::<usize, DestroyFn>(original)(renderer, flags) }
}

/// Whether this game draws the others' previews: every target installed,
/// the call redirected.
pub fn installed() -> bool {
    ARMABLE.load(Ordering::Acquire) != 0 && GAME.get().is_some()
}

/// The live `CGameUI`, or 0.
fn game_ui(layout: &Layout) -> usize {
    let menu = MENU.load(Ordering::Acquire);
    if menu == 0 {
        return 0;
    }
    // SAFETY: the menu whose frame ran last, live for the game's life; the
    // field is the pointer its own StartGame stores, read whole.
    unsafe { std::ptr::read_volatile((menu + layout.game_ui) as *const usize) }
}

/// Reads the pointer at `object + offset`.
///
/// # Safety
///
/// `object` is live and has a pointer field at `offset`.
unsafe fn pointer_at(object: usize, offset: usize) -> usize {
    // SAFETY: the caller's.
    unsafe { std::ptr::read_volatile((object + offset) as *const usize) }
}

/// Arms this thread: the next `makeProposalData` it calls draws for `from`.
pub fn arm(from: PlayerId) -> Result<(), String> {
    if !installed() {
        return Err("this build cannot draw the others' previews".into());
    }
    if !on_gui_thread() {
        return Err("only the GUI thread draws".into());
    }
    ARMED.with(|armed| armed.set(Some(from)));
    OUTCOME.with(|outcome| outcome.replace(None));
    Ok(())
}

/// Disarms this thread: what the armed call came to, or `None` when the
/// game made no `ProposalData` (its conversion failed first).
pub fn disarm() -> Option<Result<(), String>> {
    ARMED.with(|armed| armed.set(None));
    OUTCOME.with(|outcome| outcome.replace(None))
}

/// `from`'s tool shows nothing now.
pub fn hide(from: PlayerId) {
    let Some(game) = GAME.get() else { return };
    if !installed() || !on_gui_thread() {
        return;
    }
    let ui = game_ui(&game.layout);
    table().hide(game, from, ui);
    recompose();
}

/// The zero offset `AddToRenderer` takes.
static ZERO: [f32; 4] = [0.0; 4];

/// An empty `std::unordered_map<int, std::pair<ecs::Entity, float>>` as
/// MSVC lays one out, which `AddToRenderer` only reads: the game's own
/// empty one (the ProposalViewer's, 0x2aa39e3-0x2aa3aba) has its maximum
/// load factor, then its list (a 0x20-byte sentinel node linked to itself,
/// and the size), its bucket vector (two iterators a bucket, all the
/// sentinel), its mask and its bucket count.
#[repr(C)]
struct EmptyMap {
    max_load: f32,
    pad: u32,
    head: usize,
    size: usize,
    first: usize,
    last: usize,
    end: usize,
    mask: usize,
    buckets: usize,
}

/// The empty map, made once and kept for the game's life.
fn empty_map() -> usize {
    static MAP: OnceLock<usize> = OnceLock::new();
    *MAP.get_or_init(|| {
        const BUCKETS: usize = 8;
        let node: &'static mut [usize; 4] = Box::leak(Box::new([0; 4]));
        let head = node.as_mut_ptr() as usize;
        node[0] = head;
        node[1] = head;
        let vec: &'static mut [usize; 2 * BUCKETS] = Box::leak(Box::new([head; 2 * BUCKETS]));
        let first = vec.as_mut_ptr() as usize;
        let map = Box::leak(Box::new(EmptyMap {
            max_load: 1.0,
            pad: 0,
            head,
            size: 0,
            first,
            last: first + 2 * BUCKETS * size_of::<usize>(),
            end: first + 2 * BUCKETS * size_of::<usize>(),
            mask: BUCKETS - 1,
            buckets: BUCKETS,
        }));
        std::ptr::from_mut(map) as usize
    })
}

/// The armed call's `ProposalData` `data`, made from `proposal` with
/// `toolkit`: drawn in `from`'s renderer.
///
/// # Safety
///
/// The arguments are those of the game's own call of `CreateProposalData`
/// in `makeProposalData`, after it returned, on the GUI thread.
unsafe fn draw_made(
    game: &Game,
    from: PlayerId,
    toolkit: usize,
    proposal: usize,
    data: usize,
) -> Result<(), String> {
    if !on_gui_thread() {
        return Err("only the GUI thread draws".into());
    }
    let layout = &game.layout;
    let ui = game_ui(layout);
    if ui == 0 {
        return Err("no world's GUI".into());
    }
    // A proposal the game calls critical ("Construction not possible") is
    // drawn too, as the game's own tools draw theirs: the street, track and
    // construction builders fill their renderer from it whatever its
    // `errorState.critical` (`ProposalData+0x570`), and AddToRenderer draws
    // that state itself (0x5e3357). Only the ProposalViewer skips it.
    // SAFETY: the live CGameUI's fields its own CreateUI set.
    let (factory, view, models) = unsafe {
        (
            pointer_at(ui, layout.factory),
            pointer_at(ui, layout.main_view),
            pointer_at(ui, layout.model_data),
        )
    };
    if models == 0 {
        return Err("the world's GUI has no model data yet".into());
    }
    let renderer = {
        let mut table = table();
        let made = table.renderer_for(game, from, ui, factory, view);
        note_ours(&table);
        made?
    };
    let heights = game.heights.is_some();
    if !heights {
        // SAFETY: the hook's own renderer, live: without the composition its
        // terrain heights stay out of the view terrain every renderer shares.
        unsafe { std::ptr::write_volatile((renderer + layout.upload) as *mut u8, 0) };
    }
    game.clear(renderer);
    // SAFETY: as the game's ProposalViewer calls it (0x2aa3b0d): its model
    // data, the toolkit and proposal the game just used, the data it made,
    // no offset and no entity map; no catchment-area job on the thread pool,
    // so nothing outlives this call.
    unsafe {
        (game.fill)(
            models,
            toolkit,
            renderer,
            proposal,
            data,
            ZERO.as_ptr() as usize,
            empty_map(),
            0,
            0,
            1,
        );
    }
    if heights {
        // The clear reset every renderer's heights, the fill uploaded this
        // one's: all of them again, this player's own tools last.
        recompose();
    } else {
        // The upload flag again: the fill may set it for its own pass.
        // SAFETY: as above.
        unsafe { std::ptr::write_volatile((renderer + layout.upload) as *mut u8, 0) };
    }
    table().mark(from, true);
    Ok(())
}

/// `makeProposalData`'s call of `CreateProposalData`, redirected: the game's
/// own call first, then, on a thread armed for a member, the drawing.
unsafe extern "C-unwind" fn evaluate_redirect(
    out: usize,
    toolkit: usize,
    r8: usize,
    proposal: usize,
    preprocess: usize,
    optional: usize,
    context: usize,
) -> usize {
    let Some(game) = GAME.get() else {
        return out;
    };
    // SAFETY: the game's call, passed through as it made it.
    let made =
        unsafe { (game.evaluate)(out, toolkit, r8, proposal, preprocess, optional, context) };
    if let Some(from) = ARMED.with(Cell::take) {
        // SAFETY: the game's own arguments, after its call returned.
        let outcome = unsafe { draw_made(game, from, toolkit, proposal, out) };
        OUTCOME.with(|slot| slot.replace(Some(outcome)));
    }
    made
}

/// `~CGameUI`'s signature as detoured: `this` and whatever else is in the
/// argument registers, passed through.
type DtorFn = unsafe extern "C-unwind" fn(usize, usize, usize, usize) -> usize;

/// `~CGameUI`, detoured: the hook's renderers of this `CGameUI` go first.
unsafe extern "C-unwind" fn game_ui_dtor(this: usize, a: usize, b: usize, c: usize) -> usize {
    TEARING.store(true, Ordering::Release);
    if let Some(game) = GAME.get() {
        // SAFETY: the CGameUI being destroyed, still whole at its
        // destructor's entry.
        let view = unsafe { pointer_at(this, game.layout.main_view) };
        let mut table = table();
        // Forgotten by the detours before they go.
        *ours() = table
            .drawn
            .iter()
            .filter(|d| d.game_ui != this)
            .map(|d| d.renderer)
            .collect();
        let gone = table.teardown(game, this, view);
        drop(table);
        // The player's tool and what the GUI drew went with it.
        crate::previews::world_gone();
        if gone > 0 {
            crate::lua::log(format!(
                "build previews: {gone} member renderer(s) taken down with the world's GUI"
            ));
        }
    }
    let original = GAME_UI_DTOR_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the destructor's trampoline, called as the game called it.
    let done = unsafe { std::mem::transmute::<usize, DtorFn>(original)(this, a, b, c) };
    // The world's tools went with it; one the destructor did not reach is
    // never composed again.
    locals().clear();
    TEARING.store(false, Ordering::Release);
    done
}

/// Installs the drawing: reads the layout, redirects the call and detours
/// `~CGameUI`. Returns the line for the log; anything missing installs
/// nothing and says why (nobody's preview is drawn).
///
/// # Safety
///
/// Every address `at` gives is what the profile names in this very build,
/// which no thread runs yet; `detour` is as `InlineDetour::install`.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn install(
    at: &dyn Fn(&str) -> Result<usize, String>,
    detour: unsafe fn(*mut u8, *const u8) -> Result<usize, String>,
) -> String {
    let why =
        |error: String| format!("the others' build previews are not drawn (fail closed): {error}");
    let code = |name: &str| -> Option<Vec<u8>> {
        let address = at(name).ok()?;
        // SAFETY: an instruction the profile resolved and checked in this
        // process's code, mapped and only read; anchors are 7 or 8 bytes.
        Some(unsafe { std::slice::from_raw_parts(address as *const u8, 8) }.to_vec())
    };
    let layout = match Layout::read(&code) {
        Ok(layout) => layout,
        Err(error) => return why(error),
    };
    let targets = (|| {
        Ok::<_, String>((
            at(CREATE_TARGET)?,
            at(ADD_TARGET)?,
            at(REMOVE_TARGET)?,
            at(CLEAR_TARGET)?,
            at(DESTROY_TARGET)?,
            at(FILL_TARGET)?,
            at(EVALUATE_TARGET)?,
            at(CALL_TARGET)?,
            at(GAME_UI_DTOR_TARGET)?,
        ))
    })();
    let (create, add, remove, mut clear, mut destroy, fill, evaluate, call, dtor) = match targets {
        Ok(targets) => targets,
        Err(error) => return why(error),
    };
    // The terrain composition: every part, or no heights at all.
    let composition = (|| -> Result<(Upload, usize), String> {
        let anchor = at(UPLOAD_FIELD.0)?;
        // SAFETY: the instruction sequence the profile resolved and checked
        // in this process's code, mapped and only read.
        let bytes = unsafe { std::slice::from_raw_parts(anchor as *const u8, UPLOAD_LEN) };
        let upload = Upload::read(bytes, anchor)
            .ok_or_else(|| format!("{} is not the upload it names", UPLOAD_FIELD.0))?;
        let apply = at(APPLY_TARGET)?;
        if upload.apply != apply {
            return Err(format!("the upload does not call {APPLY_TARGET}"));
        }
        Ok((upload, at(END_HEIGHTS_TARGET)?))
    })();
    let mut heights = None;
    let terrain = match composition {
        Err(error) => format!("their terrain is not shown ({error})"),
        Ok((upload, end_heights)) => {
            // The destructor first: a renderer noted is always forgotten.
            // SAFETY: the caller's; each detour passes its registers through.
            let detoured = unsafe {
                detour(destroy as *mut u8, destroy_detour as *const u8).and_then(|original| {
                    DESTROY_ORIGINAL.store(original, Ordering::Release);
                    destroy = original;
                    let original = detour(end_heights as *mut u8, end_heights_detour as *const u8)?;
                    END_HEIGHTS_ORIGINAL.store(original, Ordering::Release);
                    let original = detour(clear as *mut u8, clear_detour as *const u8)?;
                    CLEAR_ORIGINAL.store(original, Ordering::Release);
                    clear = original;
                    Ok(())
                })
            };
            match detoured {
                Ok(()) => {
                    // SAFETY: the address the upload calls, which is the
                    // function the profile names; its ABI as the upload's.
                    let apply = unsafe { std::mem::transmute::<usize, ApplyFn>(upload.apply) };
                    heights = Some((upload, apply));
                    "with their terrain, composed".to_string()
                }
                Err(error) => format!("their terrain is not shown (detouring: {error})"),
            }
        }
    };
    // SAFETY: each address is the function the profile names, found by its
    // signature and prologue in this very build; each type is its ABI as
    // the game's own callers use it (investigation/TPF3_BUILD_PREVIEWS_...).
    #[allow(clippy::missing_transmute_annotations)]
    let game = unsafe {
        Game {
            layout,
            create: std::mem::transmute(create),
            add: std::mem::transmute(add),
            remove: std::mem::transmute(remove),
            clear: std::mem::transmute(clear),
            destroy: std::mem::transmute(destroy),
            fill: std::mem::transmute(fill),
            evaluate: std::mem::transmute(evaluate),
            heights,
        }
    };
    // The detour first: a renderer is never made unless it can be taken
    // down with its world.
    // SAFETY: the caller's; game_ui_dtor passes the destructor's registers
    // through.
    let original = match unsafe { detour(dtor as *mut u8, game_ui_dtor as *const u8) } {
        Ok(original) => original,
        Err(error) => return why(format!("detouring {GAME_UI_DTOR_TARGET}: {error}")),
    };
    GAME_UI_DTOR_ORIGINAL.store(original, Ordering::Release);
    if GAME.set(game).is_err() {
        return why("installed twice".into());
    }
    // SAFETY: the call site the profile resolved, which no thread runs yet;
    // install checks it calls CreateProposalData, and evaluate_redirect has
    // its ABI.
    match unsafe {
        tpf3mp_hookcore::detour::CallRedirect::install(
            call as *mut u8,
            evaluate,
            evaluate_redirect as *const u8,
        )
    } {
        Ok(redirect) => std::mem::forget(redirect),
        Err(error) => {
            // The table stays empty: GAME is set but no call ever arms.
            return why(format!("redirecting {CALL_TARGET}: {error:?}"));
        }
    }
    COMPOSING.store(
        GAME.get().is_some_and(|game| game.heights.is_some()),
        Ordering::Release,
    );
    ARMABLE.store(1, Ordering::Release);
    format!(
        "the others' build previews are drawn, {terrain}: a renderer each, CGameUI +{:#x} (factory +{:#x}, main view +{:#x}, model data +{:#x}), renderer +{:#x}",
        layout.game_ui, layout.factory, layout.main_view, layout.model_data, layout.upload
    )
}

/// Set once the call is redirected: only then does [`arm`] arm.
static ARMABLE: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tpf3mp_proto::FixedBytes;

    use super::*;

    #[derive(Default)]
    struct Record {
        calls: RefCell<Vec<String>>,
        next: Cell<usize>,
    }

    impl Renderers for Record {
        fn create(&self, factory: usize) -> usize {
            self.next.set(self.next.get() + 0x100);
            let r = self.next.get();
            self.calls
                .borrow_mut()
                .push(format!("create {factory:#x} -> {r:#x}"));
            r
        }
        fn register(&self, view: usize, renderer: usize) {
            self.calls
                .borrow_mut()
                .push(format!("register {view:#x} {renderer:#x}"));
        }
        fn unregister(&self, view: usize, renderer: usize) {
            self.calls
                .borrow_mut()
                .push(format!("unregister {view:#x} {renderer:#x}"));
        }
        fn clear(&self, renderer: usize) {
            self.calls.borrow_mut().push(format!("clear {renderer:#x}"));
        }
        fn destroy(&self, renderer: usize) {
            self.calls
                .borrow_mut()
                .push(format!("destroy {renderer:#x}"));
        }
    }

    fn player(n: u8) -> PlayerId {
        PlayerId(FixedBytes([n; 32]))
    }

    #[test]
    fn each_anchor_gives_the_offset_its_instruction_uses() {
        // The build 40408 instructions (the profile's anchors).
        let anchors: [(&str, &[u8], usize); 5] = [
            (
                GAME_UI_FIELD.0,
                &[0x48, 0x89, 0x83, 0xB8, 0x06, 0x00, 0x00],
                0x6b8,
            ),
            (
                FACTORY_FIELD.0,
                &[0x49, 0x8D, 0x84, 0x24, 0x88, 0x05, 0x00, 0x00],
                0x588,
            ),
            (
                MAIN_VIEW_FIELD.0,
                &[0x49, 0x89, 0x8C, 0x24, 0xF0, 0x0B, 0x00, 0x00],
                0xbf0,
            ),
            (
                MODEL_DATA_FIELD.0,
                &[0x48, 0x8B, 0x89, 0x38, 0x05, 0x00, 0x00],
                0x538,
            ),
            (
                UPLOAD_FIELD.0,
                &[0x80, 0xB9, 0xF0, 0x00, 0x00, 0x00, 0x00],
                0xf0,
            ),
        ];
        let code = |name: &str| {
            anchors
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, bytes, _)| bytes.to_vec())
        };
        let layout = Layout::read(&code).unwrap();
        assert_eq!(
            layout,
            Layout {
                game_ui: 0x6b8,
                factory: 0x588,
                main_view: 0xbf0,
                model_data: 0x538,
                upload: 0xf0,
            }
        );
        // Another instruction there, or none: nothing is read (fail closed).
        let moved = |name: &str| {
            if name == MAIN_VIEW_FIELD.0 {
                return Some(vec![0x49, 0x89, 0x84, 0x24, 0xF0, 0x0B, 0x00, 0x00]);
            }
            code(name)
        };
        assert!(Layout::read(&moved).unwrap_err().contains("mainView"));
        assert!(Layout::read(&|_| None).is_err());
        assert_eq!(
            field_after(
                &[0x48, 0x89, 0x83, 0x00, 0x00, 0x01, 0x00],
                &[0x48, 0x89, 0x83]
            ),
            None
        );
    }

    #[test]
    fn a_member_gets_one_renderer_registered_once_and_cleared_when_hidden() {
        let ops = Record::default();
        let mut table = Table::default();
        let ann = player(1);
        let first = table.renderer_for(&ops, ann, 0xa000, 0xf00, 0xe00).unwrap();
        table.mark(ann, true);
        let again = table.renderer_for(&ops, ann, 0xa000, 0xf00, 0xe00).unwrap();
        assert_eq!(first, again);
        table.hide(&ops, ann, 0xa000);
        table.hide(&ops, ann, 0xa000);
        assert_eq!(
            *ops.calls.borrow(),
            [
                "create 0xf00 -> 0x100",
                "register 0xe00 0x100",
                "clear 0x100"
            ],
            "made and registered once; a hidden one is cleared once"
        );
    }

    #[test]
    fn a_world_taken_down_takes_its_renderers_with_it() {
        let ops = Record::default();
        let mut table = Table::default();
        table
            .renderer_for(&ops, player(1), 0xa000, 0xf00, 0xe00)
            .unwrap();
        table
            .renderer_for(&ops, player(2), 0xa000, 0xf00, 0xe00)
            .unwrap();
        ops.calls.borrow_mut().clear();
        assert_eq!(table.teardown(&ops, 0xb000, 0xe00), 0, "another world's");
        assert_eq!(table.teardown(&ops, 0xa000, 0xe00), 2);
        assert_eq!(
            *ops.calls.borrow(),
            [
                "clear 0x100",
                "unregister 0xe00 0x100",
                "destroy 0x100",
                "clear 0x200",
                "unregister 0xe00 0x200",
                "destroy 0x200"
            ],
            "cleared, off the component, then destroyed"
        );
        assert!(table.drawn.is_empty());
    }

    #[test]
    fn a_new_worlds_gui_starts_with_no_renderers_and_there_are_at_most_sixteen() {
        let ops = Record::default();
        let mut table = Table::default();
        table
            .renderer_for(&ops, player(1), 0xa000, 0xf00, 0xe00)
            .unwrap();
        // The old CGameUI is gone (its own teardown ran): never touched.
        ops.calls.borrow_mut().clear();
        table
            .renderer_for(&ops, player(1), 0xb000, 0xf00, 0xe00)
            .unwrap();
        assert_eq!(
            *ops.calls.borrow(),
            ["create 0xf00 -> 0x200", "register 0xe00 0x200"]
        );
        table.mark(player(1), true);
        for n in 2..=16 {
            table
                .renderer_for(&ops, player(n), 0xb000, 0xf00, 0xe00)
                .unwrap();
            table.mark(player(n), true);
        }
        assert!(
            table
                .renderer_for(&ops, player(17), 0xb000, 0xf00, 0xe00)
                .is_err()
        );
        assert!(
            Table::default()
                .renderer_for(&ops, player(1), 0xb000, 0, 0xe00)
                .is_err(),
            "no factory yet: nothing made"
        );
    }

    #[test]
    fn a_full_table_draws_a_new_member_in_a_renderer_that_shows_nothing() {
        let ops = Record::default();
        let mut table = Table::default();
        for n in 1..=16 {
            table
                .renderer_for(&ops, player(n), 0xa000, 0xf00, 0xe00)
                .unwrap();
            table.mark(player(n), true);
        }
        assert!(
            table
                .renderer_for(&ops, player(17), 0xa000, 0xf00, 0xe00)
                .is_err(),
            "all sixteen show a preview"
        );
        table.hide(&ops, player(3), 0xa000);
        ops.calls.borrow_mut().clear();
        let reused = table
            .renderer_for(&ops, player(17), 0xa000, 0xf00, 0xe00)
            .unwrap();
        assert_eq!(reused, 0x300, "player 3's, cleared when hidden");
        assert!(ops.calls.borrow().is_empty(), "nothing made or registered");
        assert_eq!(table.drawn.len(), 16);
        table.mark(player(17), true);
        assert!(table.drawn.iter().all(|d| d.from != player(3)));
    }

    #[test]
    fn the_empty_map_is_laid_out_as_msvc_lays_out_an_empty_unordered_map() {
        let map = empty_map() as *const usize;
        // SAFETY: the map this module made and keeps.
        unsafe {
            assert_eq!(*(map as *const f32), 1.0, "maximum load factor");
            let head = *map.add(1);
            assert_eq!(*(head as *const usize), head, "the sentinel's next");
            assert_eq!(*(head as *const usize).add(1), head, "and prev");
            assert_eq!(*map.add(2), 0, "size");
            let (first, last, end) = (*map.add(3), *map.add(4), *map.add(5));
            assert_eq!((last - first, end), (0x80, last));
            for i in 0..16 {
                assert_eq!(*(first as *const usize).add(i), head);
            }
            assert_eq!((*map.add(6), *map.add(7)), (7, 8), "mask and bucket count");
        }
    }

    /// The upload's instructions on build 40408, at 0x7bbb6a, calling
    /// ApplyBlocks at 0x396a00.
    fn upload_code() -> Vec<u8> {
        let mut code = vec![
            0x80, 0xB9, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x74, 0x1F, 0x48, 0x8B, 0x91, 0xB8, 0x01,
            0x00, 0x00, 0x48, 0x81, 0xC2, 0xF8, 0x1A, 0x00, 0x00, 0x44, 0x0F, 0xB6, 0x81, 0xF3,
            0x00, 0x00, 0x00, 0x48, 0x8B, 0x49, 0x50, 0xE8,
        ];
        let rel = (0x396a00_i64 - 0x7bbb92_i64) as i32;
        code.extend_from_slice(&rel.to_le_bytes());
        code
    }

    #[test]
    fn the_upload_gives_its_fields_and_the_function_it_calls() {
        assert_eq!(
            Upload::read(&upload_code(), 0x7bbb6a),
            Some(Upload {
                upload: 0xf0,
                state: 0x1b8,
                list: 0x1af8,
                flag: 0xf3,
                terrain: 0x50,
                apply: 0x396a00,
            })
        );
        // Anything else there: nothing composed (fail closed).
        let mut moved = upload_code();
        moved[33] = 0x4F;
        assert_eq!(Upload::read(&moved, 0x7bbb6a), None);
        let mut moved = upload_code();
        moved[35] = 0xE9;
        assert_eq!(Upload::read(&moved, 0x7bbb6a), None);
        assert_eq!(Upload::read(&upload_code()[..39], 0x7bbb6a), None);
    }

    #[derive(Default)]
    struct Ground {
        /// Renderers with heights to upload: (renderer, terrain, list, flag).
        uploads: Vec<(usize, usize, usize, u8)>,
        applied: RefCell<Vec<usize>>,
    }

    impl Heights for Ground {
        fn blocks(&self, renderer: usize) -> Option<(usize, usize, u8)> {
            self.uploads
                .iter()
                .find(|u| u.0 == renderer)
                .map(|u| (u.1, u.2, u.3))
        }
        fn apply(&self, terrain: usize, list: usize, flag: u8) {
            assert_eq!(terrain, 0x7e);
            self.applied.borrow_mut().push(list + usize::from(flag));
        }
    }

    #[test]
    fn a_reset_terrain_gets_the_members_heights_then_the_players_own_on_top() {
        let ground = Ground {
            uploads: vec![
                (0x100, 0x7e, 0x1000, 0),
                (0x300, 0x7e, 0x3000, 1),
                (0x900, 0x7e, 0x9000, 0),
            ],
            ..Ground::default()
        };
        // 0x200 is a member's renderer cleared (no heights), 0x800 one of
        // the player's tools with none.
        let applied = compose(&ground, &[0x100, 0x200, 0x300], &[0x800, 0x900]);
        assert_eq!(applied, 3);
        assert_eq!(
            *ground.applied.borrow(),
            [0x1000, 0x3001, 0x9000],
            "the members' in order, the player's own tool last, each with its flag"
        );
        assert_eq!(compose(&Ground::default(), &[0x100], &[0x900]), 0);
    }

    #[test]
    fn nothing_is_armed_where_nothing_is_installed() {
        assert!(arm(player(1)).is_err());
        assert_eq!(disarm(), None);
    }
}
