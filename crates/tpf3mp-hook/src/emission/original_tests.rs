//! The game's own emission code, relocated from the executable and run
//! (with `crate::original`): its three kernels against
//! the fused step on random grids, and its whole `Update` with and without
//! the hook, compared bit for bit, buffer for buffer.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use iced_x86::{Decoder, DecoderOptions, OpKind};

use super::*;
use crate::original::{Exe, Page};

const UPDATE_RVA: u64 = 0xaa9230;
const UPDATE_LEN: usize = 0x4b2;
const DIFFUSE_DISPATCH_RVA: u64 = 0xaa79d0;
const WIND_DISPATCH_RVA: u64 = 0xaa7b80;
const AVERAGE_DISPATCH_RVA: u64 = 0xaa7d20;
const DIFFUSE_RVA: u64 = 0xaa8570;
const WIND_RVA: u64 = 0xaa96f0;
const AVERAGE_RVA: u64 = 0xaa8190;
const CHECK_RVA: u64 = 0xaa8350;
const CHECK_LEN: usize = 0x15c;

const NEW_RVA: u64 = 0x3184230;
const DELETE_RVA: u64 = 0x318426c;
const TYPE_INDEX_RVA: u64 = 0xa4cc0;
const DATA_INDEX_RVA: u64 = 0xa4b90;
const COMPONENT_RVA: u64 = 0x144920;
const TYPE_NAME_RVA: u64 = 0x31872f9;
const PROFILE_RVA: u64 = 0x55b50;
const THREAD_POOL_RVA: u64 = 0x3056580;
/// The import slot `_invalid_parameter_noinfo_noreturn` is called through.
const INVALID_PARAMETER_SLOT_RVA: u64 = 0x3665d30;

/// One test at a time: the hook's state is global, like the game's.
static SERIAL: Mutex<()> = Mutex::new(());

extern "system" fn trap() {
    eprintln!("the game's code reached a path the test does not stand in for");
    std::process::abort();
}

extern "system" fn new(size: usize) -> *mut u8 {
    Box::leak(vec![0u64; size.div_ceil(8).max(1)].into_boxed_slice()).as_mut_ptr() as *mut u8
}

extern "system" fn delete(_at: usize, _size: usize) {}

extern "system" fn type_index(_manager: usize, _type: usize) -> i32 {
    5
}

extern "system" fn data_index(_engine: usize, entity: i32, _type: i32) -> i32 {
    entity
}

thread_local! {
    /// The components, by entity.
    static COMPONENTS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

extern "system" fn component(_manager: usize, _type: i32, index: i32) -> usize {
    COMPONENTS.with(|c| c.borrow()[index as usize])
}

extern "system" fn type_name(_type: usize, _root: usize) -> usize {
    0
}

extern "system" fn profile(_name: usize) {}

/// A thread pool of one thread: every dispatcher takes its inline path.
extern "system" fn thread_pool() -> usize {
    static POOL: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *POOL.get_or_init(|| {
        let pool = Box::leak(vec![0u64; 32].into_boxed_slice());
        // [pool+0xc8], [pool+0xd0]: one 16-byte thread record.
        pool[0xc8 / 8] = 0x1000;
        pool[0xd0 / 8] = 0x1010;
        pool.as_ptr() as usize
    })
}

/// The game's emission code in a page of the test.
struct Original {
    _page: Page,
    update: usize,
    game: Game,
    sites: [usize; 3],
}

/// Every reference the code at `rva`+`len` makes outside itself.
fn references(exe: &Exe, rva: u64, len: usize) -> (Vec<u64>, Vec<u64>) {
    let mut decoder = Decoder::with_ip(64, exe.bytes(rva, len), rva, DecoderOptions::NONE);
    let inside = |t: u64| (rva..rva + len as u64).contains(&t);
    let (mut code, mut data) = (Vec::new(), Vec::new());
    while decoder.can_decode() {
        let insn = decoder.decode();
        if insn.op_count() > 0
            && insn.op0_kind() == OpKind::NearBranch64
            && !inside(insn.near_branch_target())
        {
            code.push(insn.near_branch_target());
        }
        if insn.is_ip_rel_memory_operand() && !inside(insn.ip_rel_memory_address()) {
            data.push(insn.ip_rel_memory_address());
        }
    }
    (code, data)
}

impl Original {
    fn load(exe: &Exe) -> Self {
        let mut page = Page::new();
        let mut map: HashMap<u64, usize> = HashMap::new();
        let stubs: [(u64, usize); 8] = [
            (NEW_RVA, new as *const () as usize),
            (DELETE_RVA, delete as *const () as usize),
            (TYPE_INDEX_RVA, type_index as *const () as usize),
            (DATA_INDEX_RVA, data_index as *const () as usize),
            (COMPONENT_RVA, component as *const () as usize),
            (TYPE_NAME_RVA, type_name as *const () as usize),
            (PROFILE_RVA, profile as *const () as usize),
            (THREAD_POOL_RVA, thread_pool as *const () as usize),
        ];
        // A near jump through a slot for each stub, so calls stay rel32.
        let jump = |page: &mut Page, to: usize| {
            let slot = page.data(&(to as u64).to_le_bytes());
            let at = page.code(&[0xFF, 0x25, 0, 0, 0, 0]);
            let rel = (slot as i64 - (at as i64 + 6)) as i32;
            // SAFETY: the jump's displacement, in the page just written.
            unsafe {
                std::ptr::copy_nonoverlapping(rel.to_le_bytes().as_ptr(), (at + 2) as *mut u8, 4)
            };
            at
        };
        for (at, to) in stubs {
            let j = jump(&mut page, to);
            map.insert(at, j);
        }
        let trap_jump = jump(&mut page, trap as *const () as usize);
        let slot = page.data(&(trap as *const () as u64).to_le_bytes());
        map.insert(INVALID_PARAMETER_SLOT_RVA, slot);

        let relocate = |page: &mut Page,
                        map: &mut HashMap<u64, usize>,
                        rva: u64,
                        len: usize|
         -> (usize, Vec<(usize, usize)>) {
            let (code, data) = references(exe, rva, len);
            for t in code {
                map.entry(t).or_insert(trap_jump);
            }
            for t in data {
                map.entry(t).or_insert_with(|| {
                    let bytes = exe
                        .try_bytes(t, 32)
                        .map_or([0u8; 32].to_vec(), <[u8]>::to_vec);
                    page.data(&bytes)
                });
            }
            let snapshot = map.clone();
            page.relocate(exe, rva, len, &|t| snapshot.get(&t).copied())
        };
        let code = |what: &str| CODE.iter().find(|c| c.what.contains(what)).unwrap();
        let diffuse = relocate(&mut page, &mut map, DIFFUSE_RVA, code("Diffuse kernel").len).0;
        let wind = relocate(&mut page, &mut map, WIND_RVA, code("Wind kernel").len).0;
        let average = relocate(&mut page, &mut map, AVERAGE_RVA, code("Average kernel").len).0;
        let check = relocate(&mut page, &mut map, CHECK_RVA, CHECK_LEN).0;
        map.insert(DIFFUSE_RVA, diffuse);
        map.insert(WIND_RVA, wind);
        map.insert(AVERAGE_RVA, average);
        map.insert(CHECK_RVA, check);
        let ddisp = relocate(
            &mut page,
            &mut map,
            DIFFUSE_DISPATCH_RVA,
            code("Diffuse dispatcher").len,
        )
        .0;
        let wdisp = relocate(
            &mut page,
            &mut map,
            WIND_DISPATCH_RVA,
            code("Wind dispatcher").len,
        )
        .0;
        let adisp = relocate(
            &mut page,
            &mut map,
            AVERAGE_DISPATCH_RVA,
            code("Average dispatcher").len,
        )
        .0;
        map.insert(DIFFUSE_DISPATCH_RVA, ddisp);
        map.insert(WIND_DISPATCH_RVA, wdisp);
        map.insert(AVERAGE_DISPATCH_RVA, adisp);
        let (update, offsets) = relocate(&mut page, &mut map, UPDATE_RVA, UPDATE_LEN);
        let site = |game_offset: i64| {
            let new = offsets
                .iter()
                .find(|(old, _)| *old as i64 == game_offset)
                .unwrap()
                .1;
            update + new
        };
        Self {
            _page: page,
            update,
            game: Game {
                diffuse_dispatch: ddisp,
                wind_dispatch: wdisp,
                average_dispatch: adisp,
                diffuse_kernel: diffuse,
                wind_kernel: wind,
                average_kernel: average,
            },
            sites: [site(DIFFUSE_CALL), site(WIND_CALL), site(AVERAGE_CALL)],
        }
    }
}

/// A random number source the tests can repeat.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 1_000_001) as f32 / 1_000_000.0
    }

    /// A concentration: mostly ordinary, sometimes zero, negative zero,
    /// denormal, huge, negative or infinite.
    fn value(&mut self, specials: bool) -> f32 {
        let roll = self.next() % 1000;
        if specials {
            match roll {
                0..=9 => return 0.0,
                10..=14 => return -0.0,
                15..=19 => return f32::from_bits(1 + (self.next() % 0x7f_ffff) as u32),
                20..=22 => return 3.0e38,
                23..=27 => return -self.unit() * 5.0,
                28 => return f32::INFINITY,
                _ => {}
            }
        }
        let exp = (self.next() % 40) as i32 - 30;
        self.unit() * 10f32.powi(exp)
    }

    /// A border cell: within FLT_EPSILON of zero, as the game asserts.
    fn border(&mut self) -> f32 {
        match self.next() % 4 {
            0 => 0.0,
            1 => -0.0,
            2 => f32::from_bits(1 + (self.next() % 0x7f_ffff) as u32),
            _ => (self.unit() - 0.5) * 1e-8,
        }
    }

    fn grid(&mut self, w: usize, h: usize, specials: bool) -> Vec<f32> {
        let mut v = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                v[y * w + x] = if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    self.border()
                } else {
                    self.value(specials)
                };
            }
        }
        v
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|f| f.to_bits()).collect()
}

/// Random step parameters within the game's asserts; `pollution` adds a
/// wind under one grid point per step.
type Inputs = (f32, f32, f32, f32, Option<[f32; 2]>, [f32; 2]);

fn random_plan(rng: &mut Rng, pollution: bool) -> Inputs {
    loop {
        let a = rng.unit() * 0.25;
        let b = rng.unit() * 0.01;
        let c = rng.unit();
        let dt = [0.2f32, 0.2, 0.2, 0.21, 0.19][(rng.next() % 5) as usize];
        let gps = [16.0f32, [16.0, 8.0, 12.5][(rng.next() % 3) as usize]];
        let wind = pollution.then(|| {
            let mut axis = |g: f32| {
                let max = g / (3.0 * dt);
                match rng.next() % 6 {
                    0 => 0.0,
                    1 => -0.0,
                    _ => (rng.unit() * 2.0 - 1.0) * max * 0.99,
                }
            };
            [axis(gps[0]), axis(gps[1])]
        });
        if fused::diffuse_weights(a, b, dt).is_some()
            && wind.is_none_or(|v| Wind::new(v, gps, dt).is_some())
        {
            return (a, b, c, dt, wind, gps);
        }
    }
}

fn plan_for(buffers: Buffers, (a, b, c, dt, wind, gps): Inputs) -> Plan {
    let (w1, w2) = fused::diffuse_weights(a, b, dt).unwrap();
    Plan {
        buffers,
        params: Params {
            w1,
            w2,
            c,
            wind: wind.map(|v| Wind::new(v, gps, dt).unwrap()),
        },
        a,
        b,
        dt,
        wind: wind.map(|v| (v, gps, dt)),
    }
}

#[test]
fn the_fused_step_is_the_games_kernels_bit_for_bit() {
    let Some(exe) = Exe::load() else { return };
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let original = Original::load(&exe);
    let pool = pool::Pool::new(3);
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let sizes = [
        (3, 3),
        (3, 4),
        (4, 3),
        (5, 5),
        (9, 4),
        (10, 10),
        (11, 7),
        (16, 3),
        (17, 33),
        (33, 17),
        (40, 41),
        (97, 51),
        (130, 66),
        (258, 129),
    ];
    let mut compared = 0u64;
    for (case, &(w, h)) in sizes.iter().cycle().take(sizes.len() * 4).enumerate() {
        let pollution = case % 2 == 1;
        let specials = case % 3 == 0;
        let params = random_plan(&mut rng, pollution);
        let mut state = (
            rng.grid(w, h, specials),
            rng.grid(w, h, specials),
            rng.grid(w, h, specials),
        );
        for step in 0..12 {
            // Emitters add to the concentration between steps.
            for _ in 0..(w * h / 16).max(1) {
                let x = 1 + (rng.next() as usize % (w - 2));
                let y = 1 + (rng.next() as usize % (h - 2));
                state.0[y * w + x] += rng.unit() * 100.0;
            }
            let probe = Buffers {
                conc: std::ptr::null_mut(),
                avg: std::ptr::null(),
                temp: std::ptr::null_mut(),
                width: w,
                height: h,
            };
            let plan = plan_for(probe, params);
            // SAFETY: the relocated kernels; parameters within their asserts.
            let theirs = unsafe { reference_step(&original.game, &plan, w, state.clone()) };
            for (bands, threaded, lanes) in [
                (1, false, false),
                (1, false, true),
                (3, false, true),
                (64, true, true),
                (5, true, false),
            ] {
                let mut ours = state.clone();
                let buffers = Buffers {
                    conc: ours.0.as_mut_ptr(),
                    avg: ours.1.as_ptr(),
                    temp: ours.2.as_mut_ptr(),
                    width: w,
                    height: h,
                };
                let mut halo = vec![0.0; Step::halo_len(&buffers, bands)];
                // SAFETY: three distinct buffers of w*h floats.
                let fused =
                    unsafe { Step::new(buffers, plan.params, bands, lanes, &mut halo) }.unwrap();
                assert!(run_step(&fused, threaded.then_some(&pool)));
                let ours = arrange(ours, pollution);
                for (name, a, b) in [
                    ("concentration", &ours.0, &theirs.0),
                    ("average", &ours.1, &theirs.1),
                    ("temporary", &ours.2, &theirs.2),
                ] {
                    assert_eq!(
                        bits(a),
                        bits(b),
                        "{name}, {w}x{h}, pollution {pollution}, step {step}, {bands} bands, threads {threaded}, lanes {lanes}, {params:?}"
                    );
                }
                compared += (w * h * 3) as u64;
            }
            state = theirs;
        }
    }
    assert!(compared > 1_000_000);
}

#[test]
fn the_self_check_passes_on_the_games_kernels() {
    let Some(exe) = Exe::load() else { return };
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let original = Original::load(&exe);
    let mut rng = Rng(77);
    for (w, h, pollution) in [
        (50, 40, false),
        (50, 40, true),
        (7, 5, true),
        (300, 13, false),
    ] {
        let params = random_plan(&mut rng, pollution);
        let (mut c, a, mut t) = (
            rng.grid(w, h, true),
            rng.grid(w, h, false),
            rng.grid(w, h, false),
        );
        let buffers = Buffers {
            conc: c.as_mut_ptr(),
            avg: a.as_ptr(),
            temp: t.as_mut_ptr(),
            width: w,
            height: h,
        };
        let plan = plan_for(buffers, params);
        // SAFETY: the relocated kernels; buffers of w*h.
        let windows = unsafe { self_check(&original.game, &plan) }.unwrap();
        assert!((1..=3).contains(&windows));
        // A fused step one ulp off the game's weights is caught.
        let mut off = plan_for(buffers, params);
        off.params.w1 = f32::from_bits(off.params.w1.to_bits() + 1);
        // SAFETY: as above.
        let err = unsafe { self_check(&original.game, &off) }.unwrap_err();
        assert!(err.contains("differs"), "{err}");
    }
}

/// The game's `EmissionGridSystem` as `Update` sees it: the system, two
/// components, the buffers.
struct World {
    system: Vec<u64>,
    comps: [Vec<u64>; 2],
    entities: Vec<i32>,
    engine: Vec<u64>,
    buffers: Vec<Vec<f32>>,
}

impl World {
    #[allow(clippy::too_many_arguments)]
    fn new(
        rng: &mut Rng,
        w: usize,
        h: usize,
        a: f32,
        b: f32,
        c: f32,
        wind: [f32; 2],
        gps: [f32; 2],
    ) -> Self {
        let buffers: Vec<Vec<f32>> = (0..5).map(|i| rng.grid(w, h, i < 2)).collect();
        let mut world = Self {
            system: vec![0; 0x60 / 8],
            comps: [vec![0; 0x80 / 8], vec![0; 0x80 / 8]],
            entities: vec![0, 1],
            engine: vec![0; 0x100 / 8],
            buffers,
        };
        let grid = |v: &mut Vec<f32>| {
            let begin = v.as_mut_ptr();
            Grid {
                x0: -1,
                y0: -1,
                width: w as i32,
                height: h as i32,
                begin,
                // SAFETY: one past the end.
                end: unsafe { begin.add(v.len()) },
                cap: unsafe { begin.add(v.len()) },
            }
        };
        let put = |mem: &mut Vec<u64>, at: usize, bytes: &[u8]| {
            // SAFETY: inside the allocation (sized above).
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (mem.as_mut_ptr() as *mut u8).add(at),
                    bytes.len(),
                )
            }
        };
        let grid_bytes = |g: Grid| {
            // SAFETY: a plain repr(C) value.
            unsafe { std::slice::from_raw_parts((&raw const g).cast::<u8>(), 0x28) }.to_vec()
        };
        let [b0, b1, b2, b3, b4] = &mut world.buffers[..] else {
            unreachable!()
        };
        let g = [grid(b0), grid(b1), grid(b2), grid(b3), grid(b4)];
        for (k, comp) in world.comps.iter_mut().enumerate() {
            put(
                comp,
                COMP_GRID_POINT_SIZE,
                &[gps[0].to_le_bytes(), gps[1].to_le_bytes()].concat(),
            );
            put(comp, COMP_CONCENTRATION, &grid_bytes(g[2 * k]));
            put(comp, COMP_AVERAGE, &grid_bytes(g[2 * k + 1]));
            put(comp, 0x68, &(k as u32).to_le_bytes());
            put(
                comp,
                COMP_WIND,
                &[wind[0].to_le_bytes(), wind[1].to_le_bytes()].concat(),
            );
        }
        let entities = world.entities.as_ptr() as u64;
        let engine = world.engine.as_ptr() as u64;
        put(&mut world.system, 8, &entities.to_le_bytes());
        put(&mut world.system, SYSTEM_TEMP, &grid_bytes(g[4]));
        put(&mut world.system, 0x40, &engine.to_le_bytes());
        put(&mut world.system, SYSTEM_AVERAGE_C, &c.to_le_bytes());
        put(&mut world.system, SYSTEM_DECAY_B, &b.to_le_bytes());
        put(&mut world.system, SYSTEM_SPREAD_A, &a.to_le_bytes());
        world
    }

    /// Runs `Update` once.
    fn update(&mut self, original: &Original, dt: f32) {
        COMPONENTS.with(|c| {
            *c.borrow_mut() = vec![
                self.comps[0].as_ptr() as usize,
                self.comps[1].as_ptr() as usize,
            ]
        });
        type UpdateFn = unsafe extern "system" fn(usize, usize, usize, f32);
        // SAFETY: the relocated Update, on a world laid out as it reads it.
        unsafe {
            std::mem::transmute::<usize, UpdateFn>(original.update)(
                self.system.as_ptr() as usize,
                self.engine.as_ptr() as usize,
                0,
                dt,
            )
        };
    }

    /// Each grid's (buffer index, bits): noise and pollution's
    /// concentration and average, then the temporary.
    fn state(&self) -> Vec<(usize, Vec<u32>)> {
        let begin = |mem: &[u64], at: usize| {
            // SAFETY: a grid inside the allocation.
            unsafe { std::ptr::read_unaligned((mem.as_ptr() as *const u8).add(at) as *const Grid) }
                .begin
        };
        let pointers = [
            begin(&self.comps[0], COMP_CONCENTRATION),
            begin(&self.comps[0], COMP_AVERAGE),
            begin(&self.comps[1], COMP_CONCENTRATION),
            begin(&self.comps[1], COMP_AVERAGE),
            begin(&self.system, SYSTEM_TEMP),
        ];
        pointers
            .iter()
            .map(|&p| {
                let i = self
                    .buffers
                    .iter()
                    .position(|b| std::ptr::eq(b.as_ptr(), p))
                    .unwrap();
                (i, bits(&self.buffers[i]))
            })
            .collect()
    }
}

#[test]
fn the_hooked_update_leaves_every_buffer_as_the_games() {
    let Some(exe) = Exe::load() else { return };
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let original = Original::load(&exe);
    let mut rng = Rng(0x0dd_b1a5_e5ca_1ab1);
    for (case, (w, h)) in [
        (3, 3),
        (5, 4),
        (19, 23),
        (64, 50),
        (101, 99),
        (260, 300),
        (34, 600),
    ]
    .into_iter()
    .enumerate()
    {
        let (a, b, c, _, wind, gps) = random_plan(&mut rng, true);
        // Within a grid point at dt 0.4 too.
        let wind = wind.unwrap().map(|v| v * 0.45);
        let seed = rng.next();
        let mut stock = World::new(&mut Rng(seed), w, h, a, b, c, wind, gps);
        let mut hooked = World::new(&mut Rng(seed), w, h, a, b, c, wind, gps);
        assert_eq!(stock.state(), hooked.state());
        FORCE_REPLAY.store(case == 2, Ordering::SeqCst);
        let (fused_before, replayed_before) = (
            FUSED.load(Ordering::SeqCst),
            REPLAYED.load(Ordering::SeqCst),
        );
        for update in 0..8 {
            // dt 0.4 runs two steps per update.
            let dt = if update == 5 { 0.4 } else { 0.2 };
            stock.update(&original, dt);
            // SAFETY: the relocated Update's calls of the relocated
            // dispatchers; nothing runs them now.
            let redirects = unsafe { install_at(original.game, original.sites) }.unwrap();
            hooked.update(&original, dt);
            drop(redirects);
            assert!(!BROKEN.load(Ordering::SeqCst), "{w}x{h}");
            let (s, k) = (stock.state(), hooked.state());
            for (i, (s, k)) in s.iter().zip(&k).enumerate() {
                assert_eq!(s.0, k.0, "buffer {i} of {w}x{h} after update {update}");
                assert!(s.1 == k.1, "contents {i} of {w}x{h} after update {update}");
            }
        }
        FORCE_REPLAY.store(false, Ordering::SeqCst);
        // Nine steps a grid (two at dt 0.4), noise and pollution: all
        // fused, or all the game's way when forced.
        let fused = FUSED.load(Ordering::SeqCst) - fused_before;
        let replayed = REPLAYED.load(Ordering::SeqCst) - replayed_before;
        let expected = if case == 2 { (0, 18) } else { (18, 0) };
        assert_eq!((fused, replayed), expected, "{w}x{h}");
    }
}

#[test]
fn a_thread_without_the_default_mxcsr_runs_the_games_passes() {
    let Some(exe) = Exe::load() else { return };
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let original = Original::load(&exe);
    let mut rng = Rng(4242);
    let (a, b, c, _, wind, gps) = random_plan(&mut rng, true);
    let seed = rng.next();
    let mut stock = World::new(&mut Rng(seed), 40, 30, a, b, c, wind.unwrap(), gps);
    let mut hooked = World::new(&mut Rng(seed), 40, 30, a, b, c, wind.unwrap(), gps);
    let saved = fused::mxcsr();
    let ftz = (saved & fused::MXCSR_CONTROL) | 0x8040;
    let set = |value: u32| {
        // SAFETY: loads MXCSR from a local.
        unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) &value, options(nostack)) };
    };
    set(ftz);
    stock.update(&original, 0.2);
    // SAFETY: as in the test above.
    let redirects = unsafe { install_at(original.game, original.sites) }.unwrap();
    hooked.update(&original, 0.2);
    drop(redirects);
    set(saved);
    assert_eq!(stock.state(), hooked.state());
}

/// The game's update, as its pool would run it: each pass split into row
/// chunks over `threads` threads, then the swap.
fn games_way(
    game: &Game,
    plan: &Plan,
    threads: usize,
    (c, a, t): &mut (Vec<f32>, Vec<f32>, Vec<f32>),
) {
    let (w, h) = (plan.buffers.width, plan.buffers.height);
    let rows = h - 2;
    let chunk = rows.div_ceil(threads).max(48);
    let grid = |v: &mut Vec<f32>| {
        let begin = v.as_mut_ptr();
        Grid {
            x0: 0,
            y0: 0,
            width: w as i32,
            height: h as i32,
            begin,
            // SAFETY: one past the end.
            end: unsafe { begin.add(v.len()) },
            cap: unsafe { begin.add(v.len()) },
        }
    };
    let pass = |f: &(dyn Fn(i32, i32) + Sync)| {
        std::thread::scope(|s| {
            let mut r = 1;
            while r < h - 1 {
                let end = (r + chunk).min(h - 1);
                s.spawn(move || f(r as i32, end as i32));
                r = end;
            }
        });
    };
    struct P(Grid, Grid);
    // SAFETY: kernels read their source and write disjoint rows.
    unsafe impl Sync for P {}
    impl P {
        fn src(&self) -> &Grid {
            &self.0
        }
        fn dst(&self) -> Grid {
            self.1
        }
    }
    // SAFETY: the relocated kernels; grids of w*h; parameters in range.
    unsafe {
        let diffuse = std::mem::transmute::<usize, DiffuseKernel>(game.diffuse_kernel);
        let g = P(grid(c), grid(t));
        pass(&|r0, r1| {
            let mut dst = g.dst();
            diffuse(r0, r1, g.src(), &mut dst, plan.a, plan.b, plan.dt)
        });
        std::mem::swap(c, t);
        if let Some((value, gps, dt)) = plan.wind {
            let windk = std::mem::transmute::<usize, WindKernel>(game.wind_kernel);
            let g = P(grid(c), grid(t));
            pass(&|r0, r1| {
                let mut dst = g.dst();
                windk(r0, r1, g.src(), &mut dst, &gps, &value, dt)
            });
            std::mem::swap(c, t);
        }
        let average = std::mem::transmute::<usize, AverageKernel>(game.average_kernel);
        let mut comp = [0usize; 16];
        comp[(COMP_CONCENTRATION + GRID_DATA) / 8] = c.as_ptr() as usize;
        comp[(COMP_AVERAGE + GRID_DATA) / 8] = a.as_ptr() as usize;
        let comp = comp.as_ptr() as usize;
        let g = P(grid(t), grid(t));
        pass(&|r0, r1| {
            let mut dst = g.dst();
            average(r0, r1, comp, &mut dst, plan.params.c)
        });
        std::mem::swap(a, t);
    }
}

/// The speed-up on the 100 x 1000-tile map's grid (1,602 x 16,002), both
/// grids of one update: the game's kernels split over threads as its pool
/// does, against the fused step. Needs ~1.2 GB; run it with
/// `cargo test -p tpf3mp-hook --release -- --ignored emission_speed --nocapture`.
#[test]
#[ignore = "a benchmark: 1.2 GB and a few seconds"]
fn emission_speed() {
    let Some(exe) = Exe::load() else { return };
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let original = Original::load(&exe);
    let (w, h) = (1602usize, 16002usize);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_THREADS);
    let pool = pool::Pool::new(threads - 1);
    let mut rng = Rng(9);
    let mut grids: Vec<_> = (0..2)
        .map(|_| {
            (
                rng.grid(w, h, false),
                rng.grid(w, h, false),
                rng.grid(w, h, false),
            )
        })
        .collect();
    let mut copies = grids.clone();
    let plans: Vec<_> = [false, true]
        .into_iter()
        .map(|pollution| random_plan(&mut rng, pollution))
        .collect();
    let probe = Buffers {
        conc: std::ptr::null_mut(),
        avg: std::ptr::null(),
        temp: std::ptr::null_mut(),
        width: w,
        height: h,
    };
    let rounds = 6;
    let mut game_ms = Vec::new();
    let mut fused_ms = Vec::new();
    let mut halo = Vec::new();
    for _ in 0..rounds {
        let start = Instant::now();
        for (state, params) in grids.iter_mut().zip(&plans) {
            games_way(&original.game, &plan_for(probe, *params), threads, state);
        }
        game_ms.push(start.elapsed().as_secs_f64() * 1e3);
        let start = Instant::now();
        for (state, params) in copies.iter_mut().zip(&plans) {
            let buffers = Buffers {
                conc: state.0.as_mut_ptr(),
                avg: state.1.as_ptr(),
                temp: state.2.as_mut_ptr(),
                width: w,
                height: h,
            };
            let bands = (threads * BANDS_PER_THREAD).min(h / MIN_BAND_ROWS);
            halo.resize(Step::halo_len(&buffers, bands), 0.0);
            let plan = plan_for(buffers, *params);
            // SAFETY: three distinct buffers of w*h floats.
            let step = unsafe { Step::new(buffers, plan.params, bands, true, &mut halo) }.unwrap();
            assert!(run_step(&step, Some(&pool)));
            let taken = std::mem::take(state);
            *state = arrange(taken, plan.params.wind.is_some());
        }
        fused_ms.push(start.elapsed().as_secs_f64() * 1e3);
    }
    for (g, f) in grids.iter().zip(&copies) {
        assert!(bits(&g.0) == bits(&f.0) && bits(&g.1) == bits(&f.1) && bits(&g.2) == bits(&f.2));
    }
    let best = |v: &[f64]| v.iter().copied().fold(f64::MAX, f64::min);
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    eprintln!(
        "emission update, {w}x{h} noise + pollution, {threads} threads: the game's kernels {:.1} ms (best {:.1}), fused {:.1} ms (best {:.1}): {:.2}x",
        median(&game_ms),
        best(&game_ms),
        median(&fused_ms),
        best(&fused_ms),
        median(&game_ms) / median(&fused_ms)
    );
}

#[test]
fn the_recorded_code_is_the_executables() {
    let Some(exe) = Exe::load() else { return };
    let update = UPDATE_RVA as usize;
    check_code(update, &|at, len| {
        exe.try_bytes(at as u64, len).map(<[u8]>::to_vec)
    })
    .unwrap();
    let callee = |site: usize| {
        let code = exe.bytes(site as u64, 5);
        assert_eq!(code[0], 0xE8, "a call at {site:#x}");
        let rel = i32::from_le_bytes(code[1..5].try_into().unwrap());
        site.wrapping_add_signed(5 + rel as isize)
    };
    let game = Game::from_update(update);
    let at = |offset: i64| update.wrapping_add_signed(offset as isize);
    assert_eq!(callee(at(DIFFUSE_CALL)), game.diffuse_dispatch);
    assert_eq!(callee(at(WIND_CALL)), game.wind_dispatch);
    assert_eq!(callee(at(AVERAGE_CALL)), game.average_dispatch);
    // Each dispatcher's inline path calls its kernel.
    assert_eq!(callee(game.diffuse_dispatch + 0x180), game.diffuse_kernel);
    assert_eq!(callee(game.wind_dispatch + 0x16f), game.wind_kernel);
    assert_eq!(callee(game.average_dispatch + 0x148), game.average_kernel);
    assert_eq!(
        (game.diffuse_dispatch as u64, game.diffuse_kernel as u64),
        (DIFFUSE_DISPATCH_RVA, DIFFUSE_RVA)
    );
    assert_eq!(
        (game.wind_kernel as u64, game.average_kernel as u64),
        (WIND_RVA, AVERAGE_RVA)
    );
}
