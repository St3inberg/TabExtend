//! H.264 stream fix-up: make every SPS promise low-latency decoding.
//!
//! A decoder may hold decoded frames back before showing them, up to the
//! size of its picture buffer, in case later frames need to be shown first.
//! The SPS (sequence parameter set) can rule that out in its VUI
//! "bitstream restriction": `max_num_reorder_frames = 0` (never reordered)
//! and `max_dec_frame_buffering` as small as the stream allows. Some
//! encoders leave the restriction out (NVENC), others declare a larger
//! buffer than needed. TabExtend never uses B-frames, so the promise holds.
//!
//! Only the restriction is changed; every bit before it is copied as is.
//! If an SPS cannot be parsed, it is left alone.

/// Profiles whose SPS carries chroma format, bit depth and scaling lists.
const HIGH_PROFILES: [u32; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];
const NAL_SPS: u8 = 7;

/// The fields that matter here, for tests and logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpsSummary {
    pub profile_idc: u32,
    pub level_idc: u32,
    pub max_num_ref_frames: u32,
    pub width_in_mbs: u32,
    pub height_in_map_units: u32,
    /// (max_num_reorder_frames, max_dec_frame_buffering) if declared.
    pub restriction: Option<(u32, u32)>,
}

/// Bit positions found while parsing an SPS's RBSP.
struct Layout {
    summary: SpsSummary,
    /// Where `vui_parameters_present_flag` is.
    vui_flag_at: usize,
    /// Where `bitstream_restriction_flag` is, if there is a VUI.
    restriction_flag_at: Option<usize>,
    /// The restriction's leading fields, kept when rewriting it.
    restriction_head: Option<RestrictionHead>,
}

#[derive(Clone, Copy)]
struct RestrictionHead {
    motion_vectors_over_pic_boundaries: u32,
    max_bytes_per_pic_denom: u32,
    max_bits_per_mb_denom: u32,
    log2_max_mv_length_horizontal: u32,
    log2_max_mv_length_vertical: u32,
}

/// The values the spec infers when the restriction is absent.
const INFERRED_HEAD: RestrictionHead = RestrictionHead {
    motion_vectors_over_pic_boundaries: 1,
    max_bytes_per_pic_denom: 2,
    max_bits_per_mb_denom: 1,
    log2_max_mv_length_horizontal: 15,
    log2_max_mv_length_vertical: 15,
};

/// Rewrite every SPS in an Annex B access unit to promise low-latency
/// decoding. Returns None when nothing needed changing (or nothing could be
/// parsed), so the caller can keep the original buffer.
pub fn low_delay_access_unit(au: &[u8]) -> Option<Vec<u8>> {
    let mut replacements = Vec::new();
    for (start, end) in nal_ranges(au) {
        let nal = &au[start..end];
        if nal.first().map(|h| h & 0x1f) == Some(NAL_SPS)
            && let Some(rewritten) = low_delay_sps(nal)
        {
            replacements.push((start, end, rewritten));
        }
    }
    if replacements.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(au.len() + 16);
    let mut copied = 0;
    for (start, end, rewritten) in replacements {
        out.extend_from_slice(&au[copied..start]);
        out.extend_from_slice(&rewritten);
        copied = end;
    }
    out.extend_from_slice(&au[copied..]);
    Some(out)
}

/// Rewrite one SPS NAL unit (header byte included, emulation prevention
/// applied). None if it already promises low delay or cannot be parsed.
pub fn low_delay_sps(nal: &[u8]) -> Option<Vec<u8>> {
    let (&header, escaped) = nal.split_first()?;
    let rbsp = unescape(escaped);
    let layout = parse(&rbsp)?;
    let wanted = (0, layout.summary.max_num_ref_frames.max(1));
    if layout.summary.restriction == Some(wanted) {
        return None;
    }

    let mut out = BitWriter::default();
    match layout.restriction_flag_at {
        Some(at) => out.copy(&rbsp, at),
        None => {
            // No VUI: add one that declares nothing but the restriction.
            out.copy(&rbsp, layout.vui_flag_at);
            out.bit(1); // vui_parameters_present_flag
            for _ in 0..8 {
                // aspect ratio, overscan, video signal, chroma location,
                // timing, NAL HRD, VCL HRD, pic_struct: all absent
                out.bit(0);
            }
        }
    }
    let head = layout.restriction_head.unwrap_or(INFERRED_HEAD);
    out.bit(1); // bitstream_restriction_flag
    out.bit(head.motion_vectors_over_pic_boundaries);
    out.ue(head.max_bytes_per_pic_denom);
    out.ue(head.max_bits_per_mb_denom);
    out.ue(head.log2_max_mv_length_horizontal);
    out.ue(head.log2_max_mv_length_vertical);
    out.ue(wanted.0); // max_num_reorder_frames
    out.ue(wanted.1); // max_dec_frame_buffering
    out.trailing_bits();

    let mut nal = vec![header];
    nal.extend(escape(&out.into_bytes()));
    Some(nal)
}

/// Summarise an SPS NAL unit, for tests and diagnostics.
pub fn summarize_sps(nal: &[u8]) -> Option<SpsSummary> {
    parse(&unescape(nal.get(1..)?)).map(|l| l.summary)
}

fn parse(rbsp: &[u8]) -> Option<Layout> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.bits(8)?;
    r.bits(8)?; // constraint flags
    let level_idc = r.bits(8)?;
    r.ue()?; // seq_parameter_set_id
    if HIGH_PROFILES.contains(&profile_idc) {
        let chroma_format_idc = r.ue()?;
        if chroma_format_idc == 3 {
            r.bit()?; // separate_colour_plane_flag
        }
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.bit()?; // qpprime_y_zero_transform_bypass_flag
        if r.bit()? == 1 {
            // seq_scaling_matrix_present_flag
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.bit()? == 1 {
                    skip_scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            r.bit()?; // delta_pic_order_always_zero_flag
            r.se()?; // offset_for_non_ref_pic
            r.se()?; // offset_for_top_to_bottom_field
            let cycle = r.ue()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                r.se()?;
            }
        }
        2 => {}
        _ => return None,
    }
    let max_num_ref_frames = r.ue()?;
    r.bit()?; // gaps_in_frame_num_value_allowed_flag
    let width_in_mbs = r.ue()? + 1;
    let height_in_map_units = r.ue()? + 1;
    if r.bit()? == 0 {
        // frame_mbs_only_flag
        r.bit()?; // mb_adaptive_frame_field_flag
    }
    r.bit()?; // direct_8x8_inference_flag
    if r.bit()? == 1 {
        // frame_cropping_flag
        for _ in 0..4 {
            r.ue()?;
        }
    }
    let vui_flag_at = r.pos;
    let mut layout = Layout {
        summary: SpsSummary {
            profile_idc,
            level_idc,
            max_num_ref_frames,
            width_in_mbs,
            height_in_map_units,
            restriction: None,
        },
        vui_flag_at,
        restriction_flag_at: None,
        restriction_head: None,
    };
    if r.bit()? == 0 {
        return Some(layout);
    }

    // VUI parameters (Annex E.1.1).
    if r.bit()? == 1 {
        // aspect_ratio_info_present_flag
        if r.bits(8)? == 255 {
            r.bits(16)?; // sar_width
            r.bits(16)?; // sar_height
        }
    }
    if r.bit()? == 1 {
        r.bit()?; // overscan_appropriate_flag
    }
    if r.bit()? == 1 {
        // video_signal_type_present_flag
        r.bits(3)?; // video_format
        r.bit()?; // video_full_range_flag
        if r.bit()? == 1 {
            r.bits(24)?; // colour primaries, transfer, matrix
        }
    }
    if r.bit()? == 1 {
        // chroma_loc_info_present_flag
        r.ue()?;
        r.ue()?;
    }
    if r.bit()? == 1 {
        // timing_info_present_flag
        r.bits(32)?; // num_units_in_tick
        r.bits(32)?; // time_scale
        r.bit()?; // fixed_frame_rate_flag
    }
    let nal_hrd = r.bit()?;
    if nal_hrd == 1 {
        skip_hrd(&mut r)?;
    }
    let vcl_hrd = r.bit()?;
    if vcl_hrd == 1 {
        skip_hrd(&mut r)?;
    }
    if nal_hrd == 1 || vcl_hrd == 1 {
        r.bit()?; // low_delay_hrd_flag
    }
    r.bit()?; // pic_struct_present_flag
    layout.restriction_flag_at = Some(r.pos);
    if r.bit()? == 1 {
        let head = RestrictionHead {
            motion_vectors_over_pic_boundaries: r.bit()?,
            max_bytes_per_pic_denom: r.ue()?,
            max_bits_per_mb_denom: r.ue()?,
            log2_max_mv_length_horizontal: r.ue()?,
            log2_max_mv_length_vertical: r.ue()?,
        };
        let reorder = r.ue()?;
        let buffering = r.ue()?;
        layout.restriction_head = Some(head);
        layout.summary.restriction = Some((reorder, buffering));
    }
    Some(layout)
}

fn skip_scaling_list(r: &mut BitReader, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            let delta = r.se()?;
            next = (last + delta + 256).rem_euclid(256);
        }
        last = if next == 0 { last } else { next };
    }
    Some(())
}

fn skip_hrd(r: &mut BitReader) -> Option<()> {
    let cpb_cnt = r.ue()? + 1;
    if cpb_cnt > 32 {
        return None;
    }
    r.bits(4)?; // bit_rate_scale
    r.bits(4)?; // cpb_size_scale
    for _ in 0..cpb_cnt {
        r.ue()?; // bit_rate_value_minus1
        r.ue()?; // cpb_size_value_minus1
        r.bit()?; // cbr_flag
    }
    r.bits(20)?; // four 5-bit delay lengths
    Some(())
}

/// (start, end) of each NAL unit's bytes in an Annex B buffer, start codes
/// excluded.
fn nal_ranges(au: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let mut end = starts.get(n + 1).map_or(au.len(), |next| next - 3);
            while end > start && au[end - 1] == 0 {
                end -= 1; // a 4-byte start code's leading zero, or padding
            }
            (start, end)
        })
        .collect()
}

/// Remove emulation prevention bytes: `00 00 03` → `00 00`.
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Insert emulation prevention bytes so no start code appears inside.
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 4);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(bit.into())
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0u32, |acc, _| Some((acc << 1) | self.bit()?))
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        ((1u64 << zeros) - 1 + u64::from(rest)).try_into().ok()
    }

    /// Signed Exp-Golomb.
    fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u8, // bits used in the last byte
}

impl BitWriter {
    fn bit(&mut self, bit: u32) {
        if self.used == 0 {
            self.bytes.push(0);
        }
        if bit & 1 == 1 {
            *self.bytes.last_mut().expect("pushed") |= 1 << (7 - self.used);
        }
        self.used = (self.used + 1) % 8;
    }

    fn bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((value >> i) & 1);
        }
    }

    fn ue(&mut self, value: u32) {
        let v = u64::from(value) + 1;
        let len = 64 - v.leading_zeros();
        self.bits(0, len - 1);
        for i in (0..len).rev() {
            self.bit(((v >> i) & 1) as u32);
        }
    }

    /// Copy the first `count` bits of `data`.
    fn copy(&mut self, data: &[u8], count: usize) {
        for i in 0..count {
            self.bit(u32::from((data[i / 8] >> (7 - i % 8)) & 1));
        }
    }

    /// rbsp_stop_one_bit, then zeros to the byte boundary.
    fn trailing_bits(&mut self) {
        self.bit(1);
        while self.used != 0 {
            self.bit(0);
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // Real SPS headers, 1808×1130, from the encoders on the reference laptop.
    const VAAPI: &str = "2764002aac7210071023fe4ffc0004000440000003004000001e2768201096";
    const QSV: &str =
        "2764002aac2b6038811ff26022000003000200000300f1d080060a800060ae17bdf076870cb8";
    const OPENH264: &str = "6742c02a8c68071023fe4807844235";
    const NVENC: &str = "6742c02a9590071023fe4c05a808080a000007d00003a98108";

    fn check_rewrite(hex: &str) -> SpsSummary {
        let original = unhex(hex);
        let before = summarize_sps(&original).expect("parses");
        let after = match low_delay_sps(&original) {
            Some(rewritten) => {
                // The NAL header byte is kept.
                assert_eq!(rewritten[0], original[0]);
                summarize_sps(&rewritten).expect("rewritten SPS parses")
            }
            None => before.clone(),
        };
        let refs = before.max_num_ref_frames.max(1);
        assert_eq!(after.restriction, Some((0, refs)), "{hex}");
        // Everything before the restriction is unchanged.
        assert_eq!(
            SpsSummary {
                restriction: None,
                ..after.clone()
            },
            SpsSummary {
                restriction: None,
                ..before
            }
        );
        after
    }

    #[test]
    fn vaapi_buffer_is_shrunk_to_its_reference_frames() {
        let before = summarize_sps(&unhex(VAAPI)).unwrap();
        assert_eq!((before.profile_idc, before.max_num_ref_frames), (100, 3));
        assert_eq!(before.restriction, Some((0, 4)));
        assert_eq!(check_rewrite(VAAPI).restriction, Some((0, 3)));
        assert_eq!((before.width_in_mbs, before.height_in_map_units), (113, 71));
    }

    #[test]
    fn qsv_already_minimal_after_parse() {
        let before = summarize_sps(&unhex(QSV)).unwrap();
        assert_eq!(before.restriction, Some((0, 2)));
        assert_eq!(before.max_num_ref_frames, 2);
        assert!(low_delay_sps(&unhex(QSV)).is_none(), "nothing to change");
        check_rewrite(QSV);
    }

    #[test]
    fn openh264_is_left_alone() {
        assert!(low_delay_sps(&unhex(OPENH264)).is_none());
        check_rewrite(OPENH264);
    }

    #[test]
    fn nvenc_gets_a_restriction() {
        let before = summarize_sps(&unhex(NVENC)).unwrap();
        assert_eq!(before.restriction, None);
        assert_eq!(check_rewrite(NVENC).restriction, Some((0, 3)));
    }

    #[test]
    fn sps_without_vui_gets_one() {
        // Baseline, 1808×1136, one reference frame, no VUI.
        let mut w = BitWriter::default();
        w.bits(66, 8);
        w.bits(0xc0, 8);
        w.bits(42, 8);
        w.ue(0); // sps id
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(2); // poc type
        w.ue(1); // max_num_ref_frames
        w.bit(0); // gaps
        w.ue(112);
        w.ue(70);
        w.bit(1); // frame_mbs_only
        w.bit(1); // direct_8x8
        w.bit(0); // cropping
        w.bit(0); // no VUI
        w.trailing_bits();
        let mut nal = vec![0x67];
        nal.extend(escape(&w.into_bytes()));
        assert_eq!(summarize_sps(&nal).unwrap().restriction, None);
        assert_eq!(check_rewrite(&hex(&nal)).restriction, Some((0, 1)));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn access_unit_keeps_other_nal_units_byte_for_byte() {
        let pps = [0x68, 0xee, 0x3c, 0x80];
        let slice = [0x65, 0x88, 0x84, 0x00, 0x00, 0x03, 0x01, 0x42];
        let mut au = vec![0, 0, 0, 1];
        au.extend(unhex(VAAPI));
        au.extend([0, 0, 0, 1]);
        au.extend(pps);
        au.extend([0, 0, 1]);
        au.extend(slice);

        let out = low_delay_access_unit(&au).expect("SPS changed");
        let ranges = nal_ranges(&out);
        assert_eq!(ranges.len(), 3);
        let nal = |i: usize| &out[ranges[i].0..ranges[i].1];
        assert_eq!(summarize_sps(nal(0)).unwrap().restriction, Some((0, 3)));
        assert_eq!(nal(1), pps);
        assert_eq!(nal(2), slice);
        assert!(out.starts_with(&[0, 0, 0, 1]));
    }

    #[test]
    fn access_unit_without_sps_is_untouched() {
        let au = [0, 0, 0, 1, 0x41, 0x9a, 0x00, 0x11];
        assert!(low_delay_access_unit(&au).is_none());
    }

    #[test]
    fn escaping_round_trips_and_removes_start_codes() {
        let raw = [0, 0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 7, 0, 0];
        let escaped = escape(&raw);
        assert!(!escaped.windows(3).any(|w| w == [0, 0, 1] || w == [0, 0, 0]));
        assert_eq!(unescape(&escaped), raw);
    }

    #[test]
    fn exp_golomb_round_trips() {
        let mut w = BitWriter::default();
        let values = [0, 1, 2, 3, 7, 8, 255, 65_535, 1 << 20];
        for v in values {
            w.ue(v);
        }
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        for v in values {
            assert_eq!(r.ue(), Some(v));
        }
    }

    #[test]
    fn garbage_never_panics_or_rewrites() {
        assert!(low_delay_sps(&[]).is_none());
        assert!(low_delay_sps(&[0x67]).is_none());
        let mut seed: u32 = 0xdead_beef;
        for _ in 0..5_000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let len = (seed % 40) as usize;
            let mut nal = vec![0x67];
            nal.extend((0..len).map(|i| (seed.rotate_left(i as u32) & 0xff) as u8));
            if let Some(out) = low_delay_sps(&nal) {
                // Anything it does rewrite must parse back with the promise.
                let summary = summarize_sps(&out).expect("output parses");
                assert_eq!(summary.restriction.map(|r| r.0), Some(0));
            }
        }
    }
}
