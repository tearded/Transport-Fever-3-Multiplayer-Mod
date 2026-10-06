//! One emission grid step, fused, with the game's per-cell arithmetic.
//!
//! The game runs a step as up to three full-grid passes, each reading one
//! buffer and writing the shared temporary, then swapping
//! (investigation/TF3_SIM_COST_2026-10-05.md §1):
//!
//! ```text
//! noise:     D = Diffuse(C) -> T; swap(C,T);                         A = Average(C, V) -> T; swap(V,T)
//! pollution: D = Diffuse(C) -> T; swap(C,T); W = Wind(C) -> T; swap(C,T); A = Average(C, V) -> T; swap(V,T)
//! ```
//!
//! Every kernel writes only the inner cells (rows and columns 1..n-2) and
//! never the border ring, so the borders travel with their buffers. With
//! `Pc`, `Pa`, `Pt` the buffers the concentration, the average and the
//! temporary held before the step, the step ends with
//!
//! - noise: concentration `Pt` (D inside, `Pt`'s border), average `Pc`
//!   (A inside, `Pc`'s border), temporary `Pa` (untouched);
//! - pollution: concentration `Pc` (W inside), average `Pt` (A inside),
//!   temporary `Pa` (untouched); D lived in `Pt` and is overwritten by A.
//!
//! Here the step is one pass over row bands. Each band reads `Pc` and `Pa`
//! once and writes its rows of the two outputs, keeping the Diffuse rows
//! Wind needs in a three-row ring, so a cell is read and written about half
//! as often as by the three passes, and the arithmetic runs eight cells at
//! a time (AVX) where the game runs one. Each lane does exactly the game's
//! scalar operations in the game's order (`vmulss`/`vaddss`/`vdivss`, no
//! FMA, no reassociation), so every cell is bit-identical.
//!
//! The outputs overwrite `Pc`, which the bands also read. Before any band
//! writes, each band copies the up to four rows outside it that it reads
//! (two above, two below: Diffuse reaches one row, Wind one more), and
//! within a band the rows are written only after the last read of their
//! old values. So every band sees exactly the step's inputs, whatever the
//! number of bands, threads or their order: the output depends on nothing
//! else (the tests run 1 to 64 bands against the game's own kernels).

#![allow(unsafe_code)]

/// Diffuse's `+ 1e-15` (`0x1436fd270`): the same bits.
pub const BIAS: f32 = f32::from_bits(0x2690_1d7d);

/// MXCSR as Windows starts every thread: round to nearest, all exceptions
/// masked, no flush-to-zero, no denormals-are-zero. The status flags
/// (bits 0-5) are ignored.
pub const MXCSR_DEFAULT: u32 = 0x1f80;
/// The control bits of MXCSR: rounding, masks, FTZ and DAZ.
pub const MXCSR_CONTROL: u32 = 0xffc0;

/// Diffuse's weights as the kernel computes them: `w1 = a*5*dt` for each
/// neighbour, `w2 = (1 - 4a - b)*5*dt` for the cell. `None` where the
/// kernel's own check (`w1 >= 0 && w2 >= 0`) fails, so the game's code
/// runs and asserts as it would.
pub fn diffuse_weights(a: f32, b: f32, dt: f32) -> Option<(f32, f32)> {
    let w1 = (a * 5.0) * dt;
    let w2 = (((1.0 - a * 4.0) - b) * 5.0) * dt;
    // `vcomiss; jb`: NaN fails too.
    if w2 >= 0.0 && w1 >= 0.0 {
        Some((w1, w2))
    } else {
        None
    }
}

/// Wind's bilinear weights, as its kernel derives them from the wind, the
/// grid point size and dt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wind {
    pub c5: f32,
    pub c6: f32,
    pub c7: f32,
    pub c8: f32,
    /// `gy * gx`, each cell's divisor.
    pub area: f32,
    /// -1 when the column offset is negative, else 0.
    pub ox0: isize,
    /// -1 when the row offset is negative, else 0.
    pub oy0: isize,
}

impl Wind {
    /// `None` where the kernel's check (each axis's step shorter than a
    /// grid point) fails, so the game's code runs and asserts.
    pub fn new(wind: [f32; 2], grid_point_size: [f32; 2], dt: f32) -> Option<Self> {
        let ivx = (-wind[0] * 3.0) * dt;
        let ivy = (-wind[1] * 3.0) * dt;
        let (gx, gy) = (grid_point_size[0], grid_point_size[1]);
        // `vcomiss g, |iv|; jbe assert`.
        if !(gx > ivx.abs() && gy > ivy.abs()) {
            return None;
        }
        let area = gy * gx;
        let sx = 0.0 > ivx;
        let sy = 0.0 > ivy;
        let c6 = if sx { gx + ivx } else { ivx };
        let c5 = if sy { gy + ivy } else { ivy };
        Some(Self {
            c5,
            c6,
            c7: gx - c6,
            c8: gy - c5,
            area,
            ox0: if sx { -1 } else { 0 },
            oy0: if sy { -1 } else { 0 },
        })
    }
}

/// What one step computes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub w1: f32,
    pub w2: f32,
    /// Average's `c` (the old average's weight).
    pub c: f32,
    /// `Some` for the pollution grid, which Wind also moves.
    pub wind: Option<Wind>,
}

/// The game's Diffuse, one cell: `((((w2*C + w1*W) + w1*E) + w1*N) + w1*S) + 1e-15`.
#[inline(always)]
pub fn diffuse_cell(w1: f32, w2: f32, c: f32, west: f32, east: f32, up: f32, down: f32) -> f32 {
    ((((w2 * c + w1 * west) + w1 * east) + w1 * up) + w1 * down) + BIAS
}

/// The game's Wind, one cell, from the four corners: A = (x0, y0), B =
/// (x1, y0), C = (x0, y1), D = (x1, y1).
#[inline(always)]
pub fn wind_cell(p: &Wind, a: f32, b: f32, c: f32, d: f32) -> f32 {
    ((((p.c6 * b) * p.c8 + (p.c7 * a) * p.c8) + (p.c7 * c) * p.c5) + (p.c6 * d) * p.c5) / p.area
}

/// The game's Average, one cell: `(1 - c)*conc + c*avg`.
#[inline(always)]
pub fn average_cell(one_minus_c: f32, c: f32, conc: f32, avg: f32) -> f32 {
    one_minus_c * conc + c * avg
}

/// The three buffers of one grid, as they were before the step.
#[derive(Debug, Clone, Copy)]
pub struct Buffers {
    /// The concentration (`Pc`).
    pub conc: *mut f32,
    /// The average (`Pa`); only read.
    pub avg: *const f32,
    /// The temporary (`Pt`).
    pub temp: *mut f32,
    pub width: usize,
    pub height: usize,
}

// SAFETY: the buffers are only touched inside `Step`, whose bands write
// disjoint rows and read only rows nobody writes during that phase.
unsafe impl Send for Buffers {}
// SAFETY: as above.
unsafe impl Sync for Buffers {}

/// Row arithmetic, eight lanes at a time where the CPU has AVX (every CPU
/// the game runs on: its own code is VEX-encoded), else one cell at a time.
/// The output the step does not read again goes out with non-temporal
/// stores (no read for ownership), from the first 32-byte boundary on.
mod rows {
    use super::{Wind, average_cell, diffuse_cell, wind_cell};

    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::{
        __m256, _mm256_add_ps, _mm256_div_ps, _mm256_loadu_ps, _mm256_mul_ps, _mm256_set1_ps,
        _mm256_storeu_ps, _mm256_stream_ps,
    };

    /// Whether the eight-lane rows run.
    pub fn avx() -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    }

    /// Orders this thread's non-temporal stores before what follows.
    pub fn fence() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: SSE is part of x86-64.
        unsafe {
            std::arch::x86_64::_mm_sfence();
        }
    }

    /// The first column at or after 1 where `out` is 32-byte aligned (for
    /// non-temporal stores), at most `end`.
    #[cfg(target_arch = "x86_64")]
    fn aligned_from(out: *const f32, end: usize) -> usize {
        let mut x = 1;
        while x < end && !(out as usize + 4 * x).is_multiple_of(32) {
            x += 1;
        }
        x
    }

    /// Diffuse over a row's inner cells.
    ///
    /// # Safety
    ///
    /// `out`, `up`, `mid` and `down` hold `w >= 3` floats; `out` overlaps
    /// none of the others.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn diffuse(
        avx: bool,
        out: *mut f32,
        up: *const f32,
        mid: *const f32,
        down: *const f32,
        w: usize,
        w1: f32,
        w2: f32,
    ) {
        let cell = |x: usize| {
            // SAFETY: 1 <= x <= w-2, inside every row.
            unsafe {
                *out.add(x) = diffuse_cell(
                    w1,
                    w2,
                    *mid.add(x),
                    *mid.add(x - 1),
                    *mid.add(x + 1),
                    *up.add(x),
                    *down.add(x),
                );
            }
        };
        let mut x = 1;
        #[cfg(target_arch = "x86_64")]
        if avx {
            // SAFETY: the caller's; AVX is present.
            x = unsafe { diffuse_avx(out, up, mid, down, w, w1, w2) };
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = avx;
        while x + 1 < w {
            cell(x);
            x += 1;
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    unsafe fn diffuse_avx(
        out: *mut f32,
        up: *const f32,
        mid: *const f32,
        down: *const f32,
        w: usize,
        w1: f32,
        w2: f32,
    ) -> usize {
        let (vw1, vw2) = (_mm256_set1_ps(w1), _mm256_set1_ps(w2));
        let bias = _mm256_set1_ps(super::BIAS);
        let mut x = 1;
        while x + 8 < w {
            // SAFETY: x-1 .. x+8 <= w-1, inside every row (the caller's).
            unsafe {
                let t = diffuse8(
                    vw1,
                    vw2,
                    bias,
                    _mm256_loadu_ps(mid.add(x)),
                    _mm256_loadu_ps(mid.add(x - 1)),
                    _mm256_loadu_ps(mid.add(x + 1)),
                    _mm256_loadu_ps(up.add(x)),
                    _mm256_loadu_ps(down.add(x)),
                );
                _mm256_storeu_ps(out.add(x), t);
            }
            x += 8;
        }
        x
    }

    /// [`diffuse_cell`] in eight lanes.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    #[allow(clippy::too_many_arguments)]
    fn diffuse8(
        w1: __m256,
        w2: __m256,
        bias: __m256,
        c: __m256,
        west: __m256,
        east: __m256,
        up: __m256,
        down: __m256,
    ) -> __m256 {
        let t = _mm256_add_ps(_mm256_mul_ps(w2, c), _mm256_mul_ps(w1, west));
        let t = _mm256_add_ps(t, _mm256_mul_ps(w1, east));
        let t = _mm256_add_ps(t, _mm256_mul_ps(w1, up));
        let t = _mm256_add_ps(t, _mm256_mul_ps(w1, down));
        _mm256_add_ps(t, bias)
    }

    /// [`average_cell`] in eight lanes.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    fn average8(one_minus_c: __m256, c: __m256, conc: __m256, avg: __m256) -> __m256 {
        _mm256_add_ps(_mm256_mul_ps(one_minus_c, conc), _mm256_mul_ps(c, avg))
    }

    /// Noise's row: Diffuse into `d` (streamed when `stream`), and Average
    /// of it into `a`.
    ///
    /// # Safety
    ///
    /// Every pointer holds `w >= 3` floats; neither output overlaps any
    /// other pointer.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn diffuse_average(
        avx: bool,
        stream: bool,
        d: *mut f32,
        a: *mut f32,
        up: *const f32,
        mid: *const f32,
        down: *const f32,
        avg: *const f32,
        w: usize,
        (w1, w2, c): (f32, f32, f32),
    ) {
        let one_minus_c = 1.0 - c;
        let cell = |x: usize| {
            // SAFETY: 1 <= x <= w-2, inside every row.
            unsafe {
                let v = diffuse_cell(
                    w1,
                    w2,
                    *mid.add(x),
                    *mid.add(x - 1),
                    *mid.add(x + 1),
                    *up.add(x),
                    *down.add(x),
                );
                *d.add(x) = v;
                *a.add(x) = average_cell(one_minus_c, c, v, *avg.add(x));
            }
        };
        let mut x = 1;
        #[cfg(target_arch = "x86_64")]
        if avx {
            let start = if stream { aligned_from(d, w - 1) } else { 1 };
            while x < start {
                cell(x);
                x += 1;
            }
            // SAFETY: the caller's; AVX is present; `d + x` is aligned
            // when streaming.
            x = unsafe { diffuse_average_avx(stream, x, d, a, up, mid, down, avg, w, (w1, w2, c)) };
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = (avx, stream);
        while x + 1 < w {
            cell(x);
            x += 1;
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn diffuse_average_avx(
        stream: bool,
        mut x: usize,
        d: *mut f32,
        a: *mut f32,
        up: *const f32,
        mid: *const f32,
        down: *const f32,
        avg: *const f32,
        w: usize,
        (w1, w2, c): (f32, f32, f32),
    ) -> usize {
        let (vw1, vw2) = (_mm256_set1_ps(w1), _mm256_set1_ps(w2));
        let bias = _mm256_set1_ps(super::BIAS);
        let (vc, vm) = (_mm256_set1_ps(c), _mm256_set1_ps(1.0 - c));
        while x + 8 < w {
            // SAFETY: x-1 .. x+8 <= w-1 (the caller's); `d + x` aligned
            // when streaming.
            unsafe {
                let v = diffuse8(
                    vw1,
                    vw2,
                    bias,
                    _mm256_loadu_ps(mid.add(x)),
                    _mm256_loadu_ps(mid.add(x - 1)),
                    _mm256_loadu_ps(mid.add(x + 1)),
                    _mm256_loadu_ps(up.add(x)),
                    _mm256_loadu_ps(down.add(x)),
                );
                if stream {
                    _mm256_stream_ps(d.add(x), v);
                } else {
                    _mm256_storeu_ps(d.add(x), v);
                }
                _mm256_storeu_ps(a.add(x), average8(vm, vc, v, _mm256_loadu_ps(avg.add(x))));
            }
            x += 8;
        }
        x
    }

    /// Pollution's row: Wind from the Diffuse rows `r0` (at the row
    /// offset) and `r1` (the next) into `out`, and Average of it into `a`
    /// (streamed when `stream`).
    ///
    /// # Safety
    ///
    /// Every pointer holds `w >= 3` floats; neither output overlaps any
    /// other pointer.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn wind_average(
        avx: bool,
        stream: bool,
        out: *mut f32,
        a: *mut f32,
        r0: *const f32,
        r1: *const f32,
        avg: *const f32,
        w: usize,
        p: &Wind,
        c: f32,
    ) {
        let one_minus_c = 1.0 - c;
        // Column x reads x+ox0 and x+ox0+1, inside 0..w for 1 <= x <= w-2.
        let cell = |x: usize| {
            let i = x.wrapping_add_signed(p.ox0);
            // SAFETY: i and i+1 are inside the rows (above).
            unsafe {
                let v = wind_cell(p, *r0.add(i), *r0.add(i + 1), *r1.add(i), *r1.add(i + 1));
                *out.add(x) = v;
                *a.add(x) = average_cell(one_minus_c, c, v, *avg.add(x));
            }
        };
        let mut x = 1;
        #[cfg(target_arch = "x86_64")]
        if avx {
            let start = if stream { aligned_from(a, w - 1) } else { 1 };
            while x < start {
                cell(x);
                x += 1;
            }
            // SAFETY: the caller's; AVX is present; `a + x` is aligned
            // when streaming.
            x = unsafe { wind_average_avx(stream, x, out, a, r0, r1, avg, w, p, c) };
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = (avx, stream);
        while x + 1 < w {
            cell(x);
            x += 1;
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn wind_average_avx(
        stream: bool,
        mut x: usize,
        out: *mut f32,
        a: *mut f32,
        r0: *const f32,
        r1: *const f32,
        avg: *const f32,
        w: usize,
        p: &Wind,
        c: f32,
    ) -> usize {
        let (c5, c6) = (_mm256_set1_ps(p.c5), _mm256_set1_ps(p.c6));
        let (c7, c8) = (_mm256_set1_ps(p.c7), _mm256_set1_ps(p.c8));
        let area = _mm256_set1_ps(p.area);
        let (vc, vm) = (_mm256_set1_ps(c), _mm256_set1_ps(1.0 - c));
        while x + 8 < w {
            let i = x.wrapping_add_signed(p.ox0);
            // SAFETY: i .. i+8 <= w-1 (above); `a + x` aligned when
            // streaming.
            unsafe {
                let ca = _mm256_loadu_ps(r0.add(i));
                let cb = _mm256_loadu_ps(r0.add(i + 1));
                let cc = _mm256_loadu_ps(r1.add(i));
                let cd = _mm256_loadu_ps(r1.add(i + 1));
                let t = _mm256_add_ps(
                    _mm256_mul_ps(_mm256_mul_ps(c6, cb), c8),
                    _mm256_mul_ps(_mm256_mul_ps(c7, ca), c8),
                );
                let t = _mm256_add_ps(t, _mm256_mul_ps(_mm256_mul_ps(c7, cc), c5));
                let t = _mm256_add_ps(t, _mm256_mul_ps(_mm256_mul_ps(c6, cd), c5));
                let v = _mm256_div_ps(t, area);
                _mm256_storeu_ps(out.add(x), v);
                let av = average8(vm, vc, v, _mm256_loadu_ps(avg.add(x)));
                if stream {
                    _mm256_stream_ps(a.add(x), av);
                } else {
                    _mm256_storeu_ps(a.add(x), av);
                }
            }
            x += 8;
        }
        x
    }
}

pub use rows::avx;

/// The rows outside a band it reads, kept before any band writes: two
/// above and two below.
const HALO: usize = 4;

/// One step over a grid, in bands.
pub struct Step<'a> {
    buffers: Buffers,
    params: Params,
    bands: usize,
    avx: bool,
    /// Non-temporal stores for the output the step does not read again.
    stream: bool,
    /// `bands * HALO` rows, borrowed from the caller for the step.
    halo: *mut f32,
    _halo: std::marker::PhantomData<&'a mut [f32]>,
}

// SAFETY: bands write disjoint rows of the buffers and disjoint slots of
// the halo, and read only rows nobody writes in the same phase (see
// `keep` and `run`); the caller orders the phases.
unsafe impl Sync for Step<'_> {}

impl<'a> Step<'a> {
    /// The rows of halo storage a step of `bands` bands needs.
    pub fn halo_len(buffers: &Buffers, bands: usize) -> usize {
        bands * HALO * buffers.width
    }

    /// A step over `buffers` (each `width * height` floats, `width >= 3`,
    /// `height >= 3`, three distinct buffers), in `bands` bands (clamped
    /// to 1 ..= the inner rows), with `halo` of [`Step::halo_len`] floats.
    ///
    /// # Safety
    ///
    /// The buffers are valid for reads and writes of `width * height`
    /// floats, distinct and not otherwise used until the step is done.
    pub unsafe fn new(
        buffers: Buffers,
        params: Params,
        bands: usize,
        avx: bool,
        halo: &'a mut [f32],
    ) -> Option<Self> {
        let (w, h) = (buffers.width, buffers.height);
        if w < 3 || h < 3 {
            return None;
        }
        let bands = bands.clamp(1, h - 2);
        if halo.len() < bands * HALO * w {
            return None;
        }
        Some(Self {
            buffers,
            params,
            bands,
            avx: avx && rows::avx(),
            stream: avx && rows::avx(),
            halo: halo.as_mut_ptr(),
            _halo: std::marker::PhantomData,
        })
    }

    pub fn bands(&self) -> usize {
        self.bands
    }

    /// Band `i`'s rows, `r0..r1`, inside 1..h-1.
    fn band(&self, i: usize) -> (usize, usize) {
        let inner = (self.buffers.height - 2) as u64;
        let at = |k: usize| 1 + (inner * k as u64 / self.bands as u64) as usize;
        (at(i), at(i + 1))
    }

    /// Phase 1 for band `i`: keeps the rows outside it that it reads.
    ///
    /// # Safety
    ///
    /// No band's phase 2 has started; each band's phase 1 runs once.
    pub unsafe fn keep(&self, i: usize) {
        let (w, h) = (self.buffers.width, self.buffers.height);
        let (r0, r1) = self.band(i);
        for (slot, row) in [r0.checked_sub(2), r0.checked_sub(1), Some(r1), Some(r1 + 1)]
            .into_iter()
            .enumerate()
        {
            let Some(row) = row.filter(|&row| row < h) else {
                continue;
            };
            // SAFETY: row < h; this band's slot of the halo, which only
            // this call writes (the storage is shared by pointer).
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.buffers.conc.add(row * w),
                    self.halo_slot(i, slot),
                    w,
                );
            }
        }
    }

    /// Band `i`'s halo row `slot`.
    fn halo_slot(&self, i: usize, slot: usize) -> *mut f32 {
        // The halo is written through this pointer by distinct bands at
        // disjoint slots, and read in phase 2 by the band that wrote it.
        // SAFETY: inside the halo (its length was checked in `new`).
        unsafe { self.halo.add((i * HALO + slot) * self.buffers.width) }
    }

    /// The step's input concentration row `k` as band `i` sees it.
    fn conc_row(&self, i: usize, r0: usize, r1: usize, k: usize) -> *const f32 {
        let slot = if k + 2 == r0 {
            0
        } else if k + 1 == r0 {
            1
        } else if k == r1 {
            2
        } else if k == r1 + 1 {
            3
        } else {
            // SAFETY: k < h, a row of the buffer.
            return unsafe { self.buffers.conc.add(k * self.buffers.width) };
        };
        self.halo_slot(i, slot)
    }

    /// Phase 2 for band `i`: computes and writes its rows.
    ///
    /// # Safety
    ///
    /// Every band's phase 1 has finished; each band's phase 2 runs once.
    pub unsafe fn run(&self, i: usize) {
        if self.params.wind.is_some() {
            // SAFETY: the caller's.
            unsafe { self.run_pollution(i) }
        } else {
            // SAFETY: the caller's.
            unsafe { self.run_noise(i) }
        }
    }

    /// Noise: D -> `Pt` (streamed), A -> `Pc`.
    unsafe fn run_noise(&self, i: usize) {
        let Buffers {
            conc,
            avg,
            temp,
            width: w,
            ..
        } = self.buffers;
        let p = self.params;
        let (r0, r1) = self.band(i);
        // The old values of the row above and of this row, which the band
        // overwrites as it goes.
        let mut above = vec![0.0f32; w];
        let mut this = vec![0.0f32; w];
        for y in r0..r1 {
            // SAFETY: rows y-1 .. y+1 of the inputs: row y and below are
            // still unwritten, `above` holds row y-1's old values.
            unsafe {
                std::ptr::copy_nonoverlapping(conc.add(y * w), this.as_mut_ptr(), w);
                let up = if y == r0 {
                    self.conc_row(i, r0, r1, y - 1)
                } else {
                    above.as_ptr()
                };
                rows::diffuse_average(
                    self.avx,
                    self.stream,
                    temp.add(y * w),
                    conc.add(y * w),
                    up,
                    this.as_ptr(),
                    self.conc_row(i, r0, r1, y + 1),
                    avg.add(y * w),
                    w,
                    (p.w1, p.w2, p.c),
                );
            }
            std::mem::swap(&mut above, &mut this);
        }
        rows::fence();
    }

    /// Pollution: D in a ring, W -> `Pc`, A -> `Pt` (streamed).
    unsafe fn run_pollution(&self, i: usize) {
        let Buffers {
            conc,
            avg,
            temp,
            width: w,
            height: h,
        } = self.buffers;
        let p = self.params;
        let Some(wind) = p.wind else { return };
        let (r0, r1) = self.band(i);
        let mut ring = vec![0.0f32; 3 * w];
        let ring_row = |j: usize| (j % 3) * w;
        // Diffuse row j into the ring: computed inside, `Pt`'s own cells on
        // the border ring (Diffuse never writes them, Wind reads them).
        let diffuse = |j: usize, ring: &mut [f32]| {
            let dst = &mut ring[ring_row(j)..ring_row(j) + w];
            // SAFETY: j < h; rows j-1 .. j+1 of the inputs, which nobody
            // has written yet (the band writes row y after D[y+1]).
            unsafe {
                let t = temp.add(j * w);
                if j == 0 || j == h - 1 {
                    std::ptr::copy_nonoverlapping(t, dst.as_mut_ptr(), w);
                } else {
                    dst[0] = *t;
                    dst[w - 1] = *t.add(w - 1);
                    rows::diffuse(
                        self.avx,
                        dst.as_mut_ptr(),
                        self.conc_row(i, r0, r1, j - 1),
                        self.conc_row(i, r0, r1, j),
                        self.conc_row(i, r0, r1, j + 1),
                        w,
                        p.w1,
                        p.w2,
                    );
                }
            }
        };
        diffuse(r0 - 1, &mut ring);
        diffuse(r0, &mut ring);
        for y in r0..r1 {
            diffuse(y + 1, &mut ring);
            let top = y.wrapping_add_signed(wind.oy0);
            // SAFETY: row y of the outputs, this band's; the ring's rows.
            unsafe {
                rows::wind_average(
                    self.avx,
                    self.stream,
                    conc.add(y * w),
                    temp.add(y * w),
                    ring.as_ptr().add(ring_row(top)),
                    ring.as_ptr().add(ring_row(top + 1)),
                    avg.add(y * w),
                    w,
                    &wind,
                    p.c,
                );
            }
        }
        rows::fence();
    }

    /// The whole step on this thread: every band's phase 1, then every
    /// band's phase 2.
    pub fn run_here(&self) {
        for i in 0..self.bands {
            // SAFETY: phase 1 for all bands before any phase 2.
            unsafe { self.keep(i) };
        }
        for i in 0..self.bands {
            // SAFETY: as above.
            unsafe { self.run(i) };
        }
    }
}

/// The calling thread's MXCSR.
#[cfg(target_arch = "x86_64")]
pub fn mxcsr() -> u32 {
    let mut value = 0u32;
    // SAFETY: stores the register into a local.
    unsafe {
        std::arch::asm!(
            "stmxcsr [{}]",
            in(reg) &mut value,
            options(nostack, preserves_flags)
        );
    }
    value
}

#[cfg(not(target_arch = "x86_64"))]
pub fn mxcsr() -> u32 {
    MXCSR_DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_the_games() {
        assert_eq!(BIAS, 1e-15f32);
        assert_eq!(
            diffuse_weights(0.2, 0.0, 0.2),
            Some((0.2f32 * 5.0 * 0.2, {
                (((1.0f32 - 0.2 * 4.0) - 0.0) * 5.0) * 0.2
            }))
        );
        assert_eq!(diffuse_weights(0.3, 0.0, 0.2), None);
        assert_eq!(diffuse_weights(f32::NAN, 0.0, 0.2), None);
        let wind = Wind::new([1.0, -0.5], [16.0, 16.0], 0.2).unwrap();
        // -wind*3*dt: x moves -0.6 (negative), y +0.3.
        assert_eq!((wind.ox0, wind.oy0), (-1, 0));
        assert_eq!(wind.c6, 16.0 + -3.0f32 * 0.2);
        assert_eq!(wind.c5, (0.5f32 * 3.0) * 0.2);
        assert_eq!(wind.area, 256.0);
        assert_eq!(Wind::new([100.0, 0.0], [16.0, 16.0], 0.2), None);
        assert_eq!(Wind::new([0.0, f32::NAN], [16.0, 16.0], 0.2), None);
    }

    /// The three passes as the game runs them, cell by cell, with swaps.
    fn passes(conc: &mut Vec<f32>, avg: &mut Vec<f32>, temp: &mut Vec<f32>, w: usize, p: &Params) {
        let h = conc.len() / w;
        let at = |x: usize, y: usize| y * w + x;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                temp[at(x, y)] = diffuse_cell(
                    p.w1,
                    p.w2,
                    conc[at(x, y)],
                    conc[at(x - 1, y)],
                    conc[at(x + 1, y)],
                    conc[at(x, y - 1)],
                    conc[at(x, y + 1)],
                );
            }
        }
        std::mem::swap(conc, temp);
        if let Some(wind) = p.wind {
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let x0 = x.wrapping_add_signed(wind.ox0);
                    let y0 = y.wrapping_add_signed(wind.oy0);
                    temp[at(x, y)] = wind_cell(
                        &wind,
                        conc[at(x0, y0)],
                        conc[at(x0 + 1, y0)],
                        conc[at(x0, y0 + 1)],
                        conc[at(x0 + 1, y0 + 1)],
                    );
                }
            }
            std::mem::swap(conc, temp);
        }
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                temp[at(x, y)] = average_cell(1.0 - p.c, p.c, conc[at(x, y)], avg[at(x, y)]);
            }
        }
        std::mem::swap(avg, temp);
    }

    fn random(seed: &mut u64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                (*seed % 1_000_000) as f32 * 1e-3
            })
            .collect()
    }

    #[test]
    fn bands_and_lanes_give_the_passes_bit_for_bit() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for (w, h) in [(3, 3), (4, 5), (9, 3), (17, 11), (40, 23), (3, 40)] {
            for wind in [None, Some([0.7, -0.4]), Some([-1.1, 2.0]), Some([0.0, 0.0])] {
                let p = Params {
                    w1: 0.2,
                    w2: 0.19,
                    c: 0.9,
                    wind: wind.map(|v| Wind::new(v, [16.0, 16.0], 0.2).unwrap()),
                };
                let (c0, a0, t0) = (
                    random(&mut seed, w * h),
                    random(&mut seed, w * h),
                    random(&mut seed, w * h),
                );
                let (mut c, mut a, mut t) = (c0.clone(), a0.clone(), t0.clone());
                passes(&mut c, &mut a, &mut t, w, &p);
                for bands in [1, 2, 3, 7, 64] {
                    for lanes in [false, true] {
                        let (mut fc, fa, mut ft) = (c0.clone(), a0.clone(), t0.clone());
                        let buffers = Buffers {
                            conc: fc.as_mut_ptr(),
                            avg: fa.as_ptr(),
                            temp: ft.as_mut_ptr(),
                            width: w,
                            height: h,
                        };
                        let mut halo = vec![0.0; Step::halo_len(&buffers, bands)];
                        // SAFETY: three distinct buffers of w*h.
                        let step =
                            unsafe { Step::new(buffers, p, bands, lanes, &mut halo) }.unwrap();
                        step.run_here();
                        let (conc, avg) = if p.wind.is_some() {
                            (&fc, &ft)
                        } else {
                            (&ft, &fc)
                        };
                        let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
                        assert_eq!(bits(conc), bits(&c), "{w}x{h} {wind:?} {bands} {lanes}");
                        assert_eq!(bits(avg), bits(&a), "{w}x{h} {wind:?} {bands} {lanes}");
                        assert_eq!(bits(&fa), bits(&t), "the temporary is the old average");
                    }
                }
            }
        }
    }

    #[test]
    fn the_test_thread_runs_the_default_mxcsr() {
        assert_eq!(mxcsr() & MXCSR_CONTROL, MXCSR_DEFAULT);
    }
}
