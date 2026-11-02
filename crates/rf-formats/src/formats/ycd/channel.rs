//! Animation channels: how one value of a track (a float, a vector, a
//! quaternion component) is stored in a sequence's packed data, and the readers
//! and writers of that data (`Rsc8AnimSequenceReader`/`Writer`).

use super::util::{bit_count, bit_count_max, linq_max, linq_min};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8AnimChannelType`.
pub const CHANNEL_TYPES: [(&str, i64); 11] = [
    ("StaticQuaternion", 0),
    ("StaticVector3", 1),
    ("StaticFloat", 2),
    ("RawFloat", 3),
    ("QuantizeFloat", 4),
    ("IndirectQuantizeFloat", 5),
    ("LinearFloat", 6),
    ("CachedQuaternion1", 7),
    ("CachedQuaternion2", 8),
    ("IncrementalFloat", 9),
    ("_Count", 10),
];
pub const CHANNEL_TYPE_COUNT: usize = 10;

/// One channel of a track. `track` and `index` place it in its sequence;
/// `data_offset` (in ints) and `frame_offset` (in bits) are where its header and
/// frame bits are, which the root motion references point to.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub track: i32,
    pub index: i32,
    pub data_offset: i32,
    pub frame_offset: i32,
    pub frame_bits: i32,
    pub data: ChannelData,
}

/// The channel types and their data. `values` are one per frame, except for
/// `IndirectQuantizeFloat` where they are the distinct values `frames` index.
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelData {
    StaticQuaternion {
        value: [f32; 4],
    },
    StaticVector3 {
        value: [f32; 3],
    },
    StaticFloat {
        value: f32,
    },
    RawFloat {
        values: Option<Vec<f32>>,
    },
    QuantizeFloat {
        quantum: f32,
        offset: f32,
        values: Option<Vec<f32>>,
        value_list: Option<Vec<u32>>,
    },
    IndirectQuantizeFloat {
        value_bits: i32,
        num_ints: i32,
        quantum: f32,
        offset: f32,
        values: Option<Vec<f32>>,
        value_list: Option<Vec<u32>>,
        frames: Option<Vec<u32>>,
    },
    LinearFloat {
        num_ints: i32,
        counts: i32,
        quantum: f32,
        offset: f32,
        bit: i32,
        count1: i32,
        count2: i32,
        count3: i32,
        values: Option<Vec<f32>>,
        value_list: Option<Vec<i32>>,
    },
    /// Types 7 (`second` false: three floats and the left-out component's index)
    /// and 8 (all four).
    CachedQuaternion {
        second: bool,
        quat_index: i32,
    },
    IncrementalFloat {
        num_ints: i32,
        counts: i32,
        quantum: f32,
        offset: f32,
        count1: i32,
        count2: i32,
        count3: i32,
        chunk_vals: Option<Vec<f32>>,
        values: Option<Vec<f32>>,
        value_list: Option<Vec<u32>>,
    },
}

impl Channel {
    /// `Rsc8AnimChannel.Create`: a new channel of a type, `None` for an unknown one.
    pub fn new(ty: i64) -> Option<Self> {
        let data = match ty {
            0 => ChannelData::StaticQuaternion { value: [0.0; 4] },
            1 => ChannelData::StaticVector3 { value: [0.0; 3] },
            2 => ChannelData::StaticFloat { value: 0.0 },
            3 => ChannelData::RawFloat { values: None },
            4 => ChannelData::QuantizeFloat { quantum: 0.0, offset: 0.0, values: None, value_list: None },
            5 => ChannelData::IndirectQuantizeFloat {
                value_bits: 0,
                num_ints: 0,
                quantum: 0.0,
                offset: 0.0,
                values: None,
                value_list: None,
                frames: None,
            },
            6 => ChannelData::LinearFloat {
                num_ints: 0,
                counts: 0,
                quantum: 0.0,
                offset: 0.0,
                bit: 0,
                count1: 0,
                count2: 0,
                count3: 0,
                values: None,
                value_list: None,
            },
            7 | 8 => ChannelData::CachedQuaternion { second: ty == 8, quat_index: 0 },
            9 => ChannelData::IncrementalFloat {
                num_ints: 0,
                counts: 0,
                quantum: 0.0,
                offset: 0.0,
                count1: 0,
                count2: 0,
                count3: 0,
                chunk_vals: None,
                values: None,
                value_list: None,
            },
            _ => return None,
        };
        let frame_bits = if ty == 3 { 32 } else { 0 };
        Some(Self { track: 0, index: 0, data_offset: 0, frame_offset: 0, frame_bits, data })
    }

    /// The `Rsc8AnimChannelType` value.
    pub fn type_id(&self) -> usize {
        match &self.data {
            ChannelData::StaticQuaternion { .. } => 0,
            ChannelData::StaticVector3 { .. } => 1,
            ChannelData::StaticFloat { .. } => 2,
            ChannelData::RawFloat { .. } => 3,
            ChannelData::QuantizeFloat { .. } => 4,
            ChannelData::IndirectQuantizeFloat { .. } => 5,
            ChannelData::LinearFloat { .. } => 6,
            ChannelData::CachedQuaternion { second: false, .. } => 7,
            ChannelData::CachedQuaternion { second: true, .. } => 8,
            ChannelData::IncrementalFloat { .. } => 9,
        }
    }

    pub fn type_name(&self) -> &'static str {
        CHANNEL_TYPES[self.type_id()].0
    }

    pub(crate) fn read_header(&mut self, r: &mut SeqReader) -> Result<()> {
        let nf = r.num_frames as usize;
        match &mut self.data {
            ChannelData::StaticQuaternion { value } => {
                let [x, y, z] = r.vec3()?;
                let w = 1.0f32 - ((x * x + y * y) + z * z);
                // Math.Max(w, 0) keeps a NaN.
                let w = if w.is_nan() || w > 0.0 { w } else { 0.0 };
                *value = [x, y, z, w.sqrt()];
            }
            ChannelData::StaticVector3 { value } => *value = r.vec3()?,
            ChannelData::StaticFloat { value } => *value = r.f32()?,
            ChannelData::RawFloat { values } => *values = Some(vec![0.0; nf]),
            ChannelData::QuantizeFloat { quantum, offset, values, value_list } => {
                self.frame_bits = r.i32()?;
                *quantum = r.f32()?;
                *offset = r.f32()?;
                *values = Some(vec![0.0; nf]);
                *value_list = Some(vec![0; nf]);
            }
            ChannelData::IndirectQuantizeFloat {
                value_bits,
                num_ints,
                quantum,
                offset,
                values,
                value_list,
                frames,
            } => {
                self.frame_bits = r.i32()?;
                *value_bits = r.i32()?;
                *num_ints = r.i32()?;
                *quantum = r.f32()?;
                *offset = r.f32()?;
                *frames = Some(vec![0; nf]);
                let Some(n0) = num_ints.wrapping_mul(32).checked_div(*value_bits) else {
                    bail!("indirect quantize float channel with {value_bits} value bits");
                };
                let n0 = n0 as i64;
                let n1 = 1u32.wrapping_shl(self.frame_bits as u32).wrapping_sub(1) as i64;
                let n = n0.min(n1);
                if n < 0 {
                    bail!("indirect quantize float channel with {n} values");
                }
                let mut vs = Vec::with_capacity(n as usize);
                let mut vl = Vec::with_capacity(n as usize);
                r.bit_position = r.position * 8;
                for _ in 0..n {
                    let bits = r.read_bits(*value_bits);
                    vs.push(bits as f32 * *quantum + *offset);
                    vl.push(bits);
                }
                *values = Some(vs);
                *value_list = Some(vl);
                r.position += *num_ints as i64 * 4;
            }
            ChannelData::LinearFloat {
                num_ints,
                counts,
                quantum,
                offset,
                bit,
                count1,
                count2,
                count3,
                values,
                value_list,
            } => {
                *num_ints = r.i32()?;
                *counts = r.i32()?;
                *quantum = r.f32()?;
                *offset = r.f32()?;
                *bit = (r.position * 8) as i32;
                *count1 = *counts & 0xFF;
                *count2 = (*counts >> 8) & 0xFF;
                *count3 = (*counts >> 16) & 0xFF;
                let stream_length = r.data.len() as i64 * 8;
                let num_frames = r.num_frames as i32;
                let chunk_size = r.chunk_size as i32;
                if chunk_size == 0 {
                    bail!("linear float channel in a sequence with a chunk size of 0");
                }
                let num_chunks = ((chunk_size + num_frames - 1) / chunk_size) as u16 as usize;
                let delta_offset = *bit as i64 + num_chunks as i64 * (*count1 + *count2) as i64;
                r.bit_position = *bit as i64;
                let mut chunk_offsets = vec![0i32; num_chunks];
                let mut chunk_values = vec![0i32; num_chunks];
                let mut frame_values = vec![0f32; num_frames as usize];
                let mut frame_bits = vec![0i32; num_frames as usize];
                for o in &mut chunk_offsets {
                    *o = if *count1 > 0 { r.read_bits(*count1) as i32 } else { 0 };
                }
                for v in &mut chunk_values {
                    *v = if *count2 > 0 { r.read_bits(*count2) as i32 } else { 0 };
                }
                for i in 0..num_chunks {
                    let doffs = chunk_offsets[i] as i64 + delta_offset;
                    let mut value = chunk_values[i];
                    let cframe = i as i32 * chunk_size;
                    r.bit_position = doffs;
                    let mut inc = 0i32;
                    for j in 0..chunk_size {
                        let frame = cframe + j;
                        if frame >= num_frames {
                            break;
                        }
                        frame_values[frame as usize] = value as f32 * *quantum + *offset;
                        frame_bits[frame as usize] = value;
                        if j + 1 >= chunk_size {
                            break;
                        }
                        let mut delta = if *count3 != 0 { r.read_bits(*count3) as i32 } else { 0 };
                        let so = r.bit_position;
                        let maxso = stream_length;
                        let mut b = 0;
                        while b == 0 {
                            b = r.read_bits(1);
                            if r.bit_position >= maxso {
                                break;
                            }
                        }
                        delta |= ((r.bit_position - so - 1) as i32).wrapping_shl(*count3 as u32);
                        if delta != 0 && r.read_bits(1) == 1 {
                            delta = delta.wrapping_neg();
                        }
                        inc = inc.wrapping_add(delta);
                        value = value.wrapping_add(inc);
                    }
                }
                *values = Some(frame_values);
                *value_list = Some(frame_bits);
                r.position -= 16;
                r.position += *num_ints as i64 * 4;
            }
            ChannelData::CachedQuaternion { .. } => {}
            ChannelData::IncrementalFloat {
                num_ints,
                counts,
                quantum,
                offset,
                count1,
                count2,
                count3,
                chunk_vals,
                values,
                value_list,
            } => {
                let start = r.position;
                *num_ints = r.i32()?;
                *counts = r.i32()?;
                *quantum = r.f32()?;
                *offset = r.f32()?;
                *count1 = *counts & 0xFF;
                *count2 = (*counts >> 8) & 0xFF;
                *count3 = (*counts as u32 >> 16) as i32;
                // The game decodes in chunks of 64 frames, whatever the header says.
                const CHUNK: usize = 64;
                let num_frames = nf;
                let num_chunks = (CHUNK + num_frames - 1) / CHUNK;
                r.bit_position = r.position * 8;
                let mut chunk_offsets = vec![0i32; num_chunks];
                let mut start_u = vec![0u32; num_chunks];
                let mut start_f = vec![0f32; num_chunks];
                for o in &mut chunk_offsets {
                    *o = r.read_bits(*count1) as i32;
                }
                for i in 0..num_chunks {
                    start_u[i] = r.read_bits(*count2);
                    start_f[i] = start_u[i] as f32 * *quantum + *offset;
                }
                let control_base = r.bit_position;
                let end = start + *num_ints as i64 * 4;
                let lanes = end - *count3 as i64 * 16;
                if *num_ints < 4 || lanes < start + 16 || end > r.data.len() as i64 {
                    bail!("invalid incremental animation channel lane storage");
                }
                let mut frame_values = vec![0f32; num_frames];
                let mut frame_bits = vec![0u32; num_frames];
                let word_at = |r: &SeqReader, i: i64| -> Result<u32> {
                    if i < 0 || i + 4 > r.data.len() as i64 {
                        bail!("incremental animation channel reads past the data");
                    }
                    let i = i as usize;
                    Ok(u32::from_le_bytes(r.data[i..i + 4].try_into().unwrap()))
                };
                for c in 0..num_chunks {
                    let base = c * CHUNK;
                    let in_chunk = CHUNK.min(num_frames - base);
                    r.bit_position = control_base + chunk_offsets[c] as i64 + c as i64 * 16;
                    let mut payload_bit = chunk_offsets[c] as i64;
                    let mut value = start_u[c] as i32;
                    let mut velocity = 0i32;
                    frame_values[base] = value as f32 * *quantum + *offset;
                    frame_bits[base] = start_u[c];
                    let mut t = 1;
                    while t < in_chunk {
                        let width = r.read_unary_length()?;
                        if width > 32 {
                            bail!("invalid incremental animation payload width");
                        }
                        let mut lane = 0;
                        while lane < 4 && t < in_chunk {
                            let mut payload = 0u32;
                            if width != 0 {
                                let word = lanes + (payload_bit / 32) * 16 + lane * 4;
                                let shift = payload_bit & 31;
                                let mut bits = word_at(r, word)? as u64;
                                if shift + width as i64 > 32 {
                                    bits |= (word_at(r, word + 16)? as u64) << 32;
                                }
                                payload = ((bits >> shift) & ((1u64 << width) - 1)) as u32;
                            }
                            let acceleration = ((payload >> 1) ^ ((payload & 1) as i32).wrapping_neg() as u32) as i32;
                            velocity = velocity.wrapping_add(acceleration);
                            value = value.wrapping_add(velocity);
                            frame_values[base + t] = value as f32 * *quantum + *offset;
                            frame_bits[base + t] = value as u32;
                            lane += 1;
                            t += 1;
                        }
                        payload_bit += width as i64;
                    }
                }
                *values = Some(frame_values);
                *value_list = Some(frame_bits);
                *chunk_vals = Some(start_f);
                r.position = end;
            }
        }
        Ok(())
    }

    pub(crate) fn read_frame(&mut self, r: &mut SeqReader) {
        let f = r.frame as usize;
        match &mut self.data {
            ChannelData::RawFloat { values: Some(v) } => v[f] = f32::from_bits(r.read_frame_bits(32)),
            ChannelData::QuantizeFloat { quantum, offset, values: Some(v), value_list: Some(l) } => {
                let bits = r.read_frame_bits(self.frame_bits);
                v[f] = bits as f32 * *quantum + *offset;
                l[f] = bits;
            }
            ChannelData::IndirectQuantizeFloat { frames: Some(fr), .. } => fr[f] = r.read_frame_bits(self.frame_bits),
            _ => {}
        }
    }

    /// The per-frame values as written in the XML. `IndirectQuantizeFloat` looks
    /// its frames up in its values, which fails (as in the C#) when one is out of range.
    pub fn frame_values(&self) -> Result<Option<Vec<f32>>> {
        Ok(match &self.data {
            ChannelData::IndirectQuantizeFloat { values, frames, .. } => {
                let Some(frames) = frames else { bail!("indirect quantize float channel without frames") };
                let values = values.as_deref().unwrap_or(&[]);
                let mut out = Vec::with_capacity(frames.len());
                for &f in frames {
                    match values.get(f as usize) {
                        Some(v) => out.push(*v),
                        None => bail!("indirect quantize float frame {f} has no value ({} values)", values.len()),
                    }
                }
                Some(out)
            }
            ChannelData::RawFloat { values }
            | ChannelData::QuantizeFloat { values, .. }
            | ChannelData::LinearFloat { values, .. }
            | ChannelData::IncrementalFloat { values, .. } => values.clone(),
            _ => None,
        })
    }

    /// `Write(MetaNodeWriter)`. `IndirectQuantizeFloat`'s values must have been
    /// checked with [`Channel::frame_values`] (the readers here do it).
    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("@type", self.type_name());
        match &self.data {
            ChannelData::StaticQuaternion { value } => w.vec4("Value", *value),
            ChannelData::StaticVector3 { value } => w.vec3("Value", *value),
            ChannelData::StaticFloat { value } => w.f32("Value", *value),
            ChannelData::CachedQuaternion { quat_index, .. } => w.i32("QuatIndex", *quat_index),
            _ => {
                let v = self.frame_values().ok().flatten();
                w.f32_array("Values", v.as_deref());
            }
        }
    }

    /// `Read(MetaNodeReader)` for a channel of type `ty`.
    pub fn read_xml(ty: Option<&str>, n: &XmlNode) -> Result<Self> {
        let t = super::util::parse_enum(ty, &CHANNEL_TYPES);
        let Some(mut c) = t.and_then(Channel::new) else { bail!("unknown animation channel type {ty:?}") };
        match &mut c.data {
            ChannelData::StaticQuaternion { value } => *value = n.vec4("Value"),
            ChannelData::StaticVector3 { value } => *value = n.vec3("Value"),
            ChannelData::StaticFloat { value } => *value = n.f32("Value", 0.0),
            ChannelData::CachedQuaternion { quat_index, .. } => *quat_index = n.i32("QuatIndex", 0),
            ChannelData::IndirectQuantizeFloat { values, frames, .. } => {
                // One value per frame, like the other channels: the distinct ones, and
                // each frame's index in them.
                let Some(all) = n.f32_array("Values") else { bail!("indirect quantize float channel without values") };
                let mut distinct: Vec<f32> = Vec::new();
                let mut fr = Vec::with_capacity(all.len());
                for v in &all {
                    // Distinct and IndexOf compare with float.Equals (NaN equals NaN).
                    let same = |x: &f32| x == v || (x.is_nan() && v.is_nan());
                    match distinct.iter().position(same) {
                        Some(i) => fr.push(i as u32),
                        None => {
                            fr.push(distinct.len() as u32);
                            distinct.push(*v);
                        }
                    }
                }
                *values = Some(distinct);
                *frames = Some(fr);
            }
            ChannelData::RawFloat { values }
            | ChannelData::QuantizeFloat { values, .. }
            | ChannelData::LinearFloat { values, .. }
            | ChannelData::IncrementalFloat { values, .. } => *values = n.f32_array("Values"),
        }
        Ok(c)
    }

    pub(crate) fn write_header(&mut self, w: &mut SeqWriter) -> Result<()> {
        match &mut self.data {
            ChannelData::StaticQuaternion { value } => w.vec3([value[0], value[1], value[2]]),
            ChannelData::StaticVector3 { value } => w.vec3(*value),
            ChannelData::StaticFloat { value } => w.f32(*value),
            ChannelData::QuantizeFloat { quantum, offset, values, .. } => {
                let values = values.as_deref().unwrap_or(&[]);
                if *quantum == 0.0 && !values.is_empty() {
                    // Values that came without a quantisation (XML): their range over 16 bits.
                    *offset = linq_min(values);
                    let range = linq_max(values) - *offset;
                    *quantum = if range > 0.0 { range / 65535.0 } else { 1.0 };
                }
                let list: Vec<u32> = values.iter().map(|v| quanta(*v, *offset, *quantum)).collect();
                self.frame_bits = bit_count_max(&list).max(1);
                w.i32(self.frame_bits);
                w.f32(*quantum);
                w.f32(*offset);
            }
            ChannelData::IndirectQuantizeFloat { value_bits, num_ints, quantum, offset, values, .. } => {
                self.frame_bits = bit_count(values.as_ref().map_or(1, |v| v.len()) as u32).max(2);
                let values = values.as_deref().unwrap_or(&[]);
                if *quantum == 0.0 && !values.is_empty() {
                    *offset = linq_min(values);
                    let range = linq_max(values) - *offset;
                    *quantum = if range > 0.0 { range / 65535.0 } else { 1.0 };
                }
                let list: Vec<u32> = values.iter().map(|v| quanta(*v, *offset, *quantum)).collect();
                *value_bits = bit_count_max(&list).max(3);
                w.reset_bitstream();
                for u in &list {
                    w.write_bits(*u, *value_bits);
                }
                *num_ints = w.bitstream.len() as i32;
                w.i32(self.frame_bits);
                w.i32(*value_bits);
                w.i32(*num_ints);
                w.f32(*quantum);
                w.f32(*offset);
                w.write_bitstream();
            }
            ChannelData::LinearFloat { num_ints, counts, quantum, offset, count1, count2, count3, values, .. } => {
                // As the C# does it, with chunks counted by 64 frames but written by 10
                // (this type isn't used by the game).
                let num_frames = w.num_frames as usize;
                let num_chunks = ((64 + num_frames as i32 - 1) / 64) as u16 as usize;
                const CHUNK: usize = 10;
                w.chunk_size = CHUNK as u8;
                let values = values.as_deref().unwrap_or(&[]);
                let list: Vec<i32> = values.iter().map(|v| ((*v - *offset) / *quantum + 0.5f32) as i32).collect();
                let mut chunk_offsets = vec![0u32; num_chunks];
                let mut chunk_values = vec![0u32; num_chunks];
                let mut chunk_deltas = vec![[0i32; CHUNK]; num_chunks];
                let mut delta_bits = vec![0u32; num_frames];
                for (i, (deltas, start)) in chunk_deltas.iter_mut().zip(chunk_values.iter_mut()).enumerate() {
                    let cframe = i * CHUNK;
                    let first = if cframe < list.len() { list.get(cframe) } else { list.first() };
                    let Some(&first) = first else { bail!("linear float channel without values") };
                    let mut cvalue = first;
                    let mut cinc = 0i32;
                    *start = cvalue as u32;
                    for (j, d) in deltas.iter_mut().enumerate().skip(1) {
                        let frame = cframe + j;
                        if frame >= num_frames {
                            break;
                        }
                        let Some(&value) = list.get(frame) else { bail!("linear float channel with too few values") };
                        let inc = value.wrapping_sub(cvalue);
                        let delta = inc.wrapping_sub(cinc);
                        if delta == i32::MIN {
                            bail!("linear float delta overflow");
                        }
                        cinc = inc;
                        cvalue = value;
                        *d = delta;
                        delta_bits[frame] = delta.unsigned_abs();
                    }
                }
                *count3 = bit_count_max(&delta_bits);
                let mut coffset = 0u32;
                for (o, deltas) in chunk_offsets.iter_mut().zip(&chunk_deltas) {
                    *o = coffset;
                    for d in &deltas[1..] {
                        coffset = coffset.wrapping_add((*count3 as u32).wrapping_add(if *d < 0 { 2 } else { 1 }));
                    }
                }
                *count1 = bit_count_max(&chunk_offsets);
                *count2 = bit_count_max(&chunk_values);
                w.reset_bitstream();
                if *count1 > 0 {
                    for o in &chunk_offsets {
                        w.write_bits(*o, *count1);
                    }
                }
                if *count2 > 0 {
                    for v in &chunk_values {
                        w.write_bits(*v, *count2);
                    }
                }
                for d in &chunk_deltas {
                    for &delta in &d[1..] {
                        w.write_bits(delta.unsigned_abs(), *count3);
                        w.write_bits(1, 1);
                        if delta < 0 {
                            w.write_bits(1, 1);
                        }
                    }
                }
                *counts = (*count1 & 0xFF) + ((*count2 & 0xFF) << 8) + ((*count3 & 0xFF) << 16);
                *num_ints = 4 + w.bitstream.len() as i32;
                w.i32(*num_ints);
                w.i32(*counts);
                w.f32(*quantum);
                w.f32(*offset);
                w.write_bitstream();
            }
            ChannelData::RawFloat { .. }
            | ChannelData::CachedQuaternion { .. }
            | ChannelData::IncrementalFloat { .. } => {}
        }
        Ok(())
    }

    pub(crate) fn write_frame(&self, w: &mut SeqWriter) -> Result<()> {
        let f = w.frame as usize;
        let missing =
            || -> crate::Error { crate::Error::Format(format!("animation channel has no value for frame {f}")) };
        match &self.data {
            ChannelData::RawFloat { values } => {
                let v = values.as_ref().and_then(|v| v.get(f)).ok_or_else(missing)?;
                w.write_frame_bits(v.to_bits(), 32);
            }
            ChannelData::QuantizeFloat { quantum, offset, values, .. } => {
                let v = values.as_ref().and_then(|v| v.get(f)).ok_or_else(missing)?;
                w.write_frame_bits(quanta(*v, *offset, *quantum), self.frame_bits);
            }
            ChannelData::IndirectQuantizeFloat { frames, .. } => {
                let v = frames.as_ref().and_then(|v| v.get(f)).ok_or_else(missing)?;
                w.write_frame_bits(*v, self.frame_bits);
            }
            _ => {}
        }
        Ok(())
    }
}

/// `GetQuanta`: the quantised value, rounded half up (and saturated, as .NET does).
fn quanta(v: f32, offset: f32, quantum: f32) -> u32 {
    ((v - offset) / quantum + 0.5f32) as u32
}

/// `Rsc8AnimSequenceReader`: reads a sequence's packed data.
pub(crate) struct SeqReader<'a> {
    pub data: &'a [u8],
    pub num_frames: u16,
    pub chunk_size: u8,
    /// The byte the channel headers are read at.
    pub position: i64,
    pub frame: i32,
    frame_offset: u32,
    frame_length: u16,
    channel_list_offset: i64,
    channel_data_offset: i64,
    /// In bits.
    channel_frame_offset: i64,
    pub bit_position: i64,
}

impl<'a> SeqReader<'a> {
    pub fn new(data: &'a [u8], num_frames: u16, chunk_size: u8, frame_offset: u32, frame_length: u16) -> Self {
        let channel_list_offset = frame_offset as i64 + frame_length as i64 * num_frames as i64;
        Self {
            data,
            num_frames,
            chunk_size,
            position: 0,
            frame: 0,
            frame_offset,
            frame_length,
            channel_list_offset,
            channel_data_offset: channel_list_offset + CHANNEL_TYPE_COUNT as i64 * 2,
            channel_frame_offset: 0,
            bit_position: 0,
        }
    }

    /// `N` bytes at `i` (as the C#'s pointer reads: past the end is zeros, but
    /// the first byte must be in the data).
    fn bytes_at<const N: usize>(&self, i: i64) -> Result<[u8; N]> {
        if i < 0 || i >= self.data.len() as i64 {
            bail!("animation sequence read at {i} is outside its {} bytes", self.data.len());
        }
        let mut b = [0u8; N];
        let i = i as usize;
        let n = N.min(self.data.len() - i);
        b[..n].copy_from_slice(&self.data[i..i + n]);
        Ok(b)
    }

    pub fn i32(&mut self) -> Result<i32> {
        let v = i32::from_le_bytes(self.bytes_at(self.position)?);
        self.position += 4;
        Ok(v)
    }
    pub fn f32(&mut self) -> Result<f32> {
        let v = f32::from_le_bytes(self.bytes_at(self.position)?);
        self.position += 4;
        Ok(v)
    }
    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        let b: [u8; 12] = self.bytes_at(self.position)?;
        self.position += 12;
        Ok([0, 4, 8].map(|i| f32::from_le_bytes(b[i..i + 4].try_into().unwrap())))
    }

    /// `GetBits`: `length` bits from `start` (least significant first), zeros past
    /// the end of the data, with the C#'s 32-bit shift wrap-around.
    fn get_bits(&self, start: i64, length: i32) -> u32 {
        if start < 0 {
            return 0;
        }
        let mut cur = (start / 8) as usize;
        let mut shift = -((start % 8) as i32);
        let mut result = 0u32;
        let mut remaining = length;
        while remaining > 0 {
            let b = if cur < self.data.len() { self.data[cur] as u32 } else { 0 };
            cur += 1;
            let sb = if shift < 0 { b >> -shift } else { b.wrapping_shl(shift as u32) };
            let bm = (1u32.wrapping_shl(remaining.min(8) as u32).wrapping_sub(1)).wrapping_shl(shift.max(0) as u32);
            result = result.wrapping_add(sb & bm);
            remaining -= 8 + shift.min(0);
            shift += 8;
        }
        result
    }

    pub fn read_bits(&mut self, length: i32) -> u32 {
        if length == 0 {
            return 0;
        }
        let v = self.get_bits(self.bit_position, length);
        self.bit_position += length as i64;
        v
    }

    fn u16_at(&self, i: i64) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes_at(i)?))
    }
    pub fn read_channel_count(&mut self) -> Result<u16> {
        let v = self.u16_at(self.channel_list_offset)?;
        self.channel_list_offset += 2;
        Ok(v)
    }
    pub fn read_channel_data_bits(&mut self) -> Result<u16> {
        let v = self.u16_at(self.channel_data_offset)?;
        self.channel_data_offset += 2;
        Ok(v)
    }
    pub fn read_channel_ref(&mut self) -> Result<ChannelRef> {
        let b: [u8; 6] = self.bytes_at(self.channel_data_offset)?;
        self.channel_data_offset += 6;
        Ok(ChannelRef {
            channel_type: b[0],
            channel_index: b[1],
            data_int_offset: u16::from_le_bytes([b[2], b[3]]),
            frame_bit_offset: u16::from_le_bytes([b[4], b[5]]),
        })
    }
    pub fn align_channel_data_offset(&mut self, count: usize) {
        let rem = count % 4;
        if rem > 0 {
            self.channel_data_offset += ((4 - rem) * 2) as i64;
        }
    }
    pub fn begin_frame(&mut self, f: i32) {
        self.frame = f;
        self.channel_frame_offset = (self.frame_offset as i64 + self.frame_length as i64 * f as i64) * 8;
    }
    pub fn read_frame_bits(&mut self, n: i32) -> u32 {
        let v = self.get_bits(self.channel_frame_offset, n);
        self.channel_frame_offset += n as i64;
        v
    }
    pub fn read_unary_length(&mut self) -> Result<u32> {
        let mut count = 0u32;
        loop {
            if self.bit_position < 0 || self.bit_position >= self.data.len() as i64 * 8 {
                bail!("truncated incremental animation channel: missing unary terminator");
            }
            if self.read_bits(1) != 0 {
                return Ok(count);
            }
            count += 1;
        }
    }
}

/// A root motion reference: which channel holds part of the root's motion, and
/// where its header (in ints) and frame bits are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelRef {
    pub channel_type: u8,
    pub channel_index: u8,
    pub data_int_offset: u16,
    pub frame_bit_offset: u16,
}

/// `Rsc8AnimSequenceWriter`: packs channels into a sequence's data.
pub(crate) struct SeqWriter {
    pub position: i32,
    pub frame: i32,
    pub num_frames: u16,
    pub chunk_size: u8,
    pub frame_length: u16,
    pub channel_list: Vec<u8>,
    pub channel_items: Vec<u8>,
    pub main: Vec<u8>,
    frame_stream: Vec<u32>,
    frame_bit_offset: i32,
    pub frames: Vec<Vec<u32>>,
    pub bitstream: Vec<u32>,
    bitstream_pos: i32,
}

impl SeqWriter {
    pub fn new(num_frames: u16) -> Self {
        Self {
            position: 0,
            frame: 0,
            num_frames,
            chunk_size: 10,
            frame_length: 0,
            channel_list: Vec::new(),
            channel_items: Vec::new(),
            main: Vec::new(),
            frame_stream: Vec::new(),
            frame_bit_offset: 0,
            frames: Vec::new(),
            bitstream: Vec::new(),
            bitstream_pos: 0,
        }
    }

    pub fn channel_list_data(&mut self, c: u16) {
        self.channel_list.extend_from_slice(&c.to_le_bytes());
    }
    pub fn channel_item_data(&mut self, c: u16) {
        self.channel_items.extend_from_slice(&c.to_le_bytes());
    }
    pub fn align_channel_item_data(&mut self, count: usize, track_count: usize) {
        let rem = count % 4;
        if rem > 0 {
            let v = (track_count << 2) as u16;
            for _ in 0..4 - rem {
                self.channel_item_data(v);
            }
        }
    }
    pub fn channel_ref(&mut self, r: &ChannelRef) {
        self.channel_items.push(r.channel_type);
        self.channel_items.push(r.channel_index);
        self.channel_items.extend_from_slice(&r.data_int_offset.to_le_bytes());
        self.channel_items.extend_from_slice(&r.frame_bit_offset.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.main.extend_from_slice(&v.to_le_bytes());
        self.position += 4;
    }
    pub fn f32(&mut self, v: f32) {
        self.main.extend_from_slice(&v.to_le_bytes());
        self.position += 4;
    }
    pub fn vec3(&mut self, v: [f32; 3]) {
        for f in v {
            self.main.extend_from_slice(&f.to_le_bytes());
        }
        self.position += 12;
    }

    pub fn begin_frame(&mut self, f: i32) {
        self.frame = f;
        self.frame_stream.clear();
        self.frame_bit_offset = 0;
    }
    pub fn write_frame_bits(&mut self, bits: u32, n: i32) {
        to_bitstream(&mut self.frame_stream, self.frame_bit_offset, bits, n);
        self.frame_bit_offset += n;
    }
    pub fn end_frame(&mut self) {
        self.frame_length = self.frame_length.max((self.frame_stream.len() * 4) as u16);
        self.frames.push(std::mem::take(&mut self.frame_stream));
    }

    pub fn reset_bitstream(&mut self) {
        self.bitstream.clear();
        self.bitstream_pos = 0;
    }
    pub fn write_bits(&mut self, bits: u32, n: i32) {
        to_bitstream(&mut self.bitstream, self.bitstream_pos, bits, n);
        self.bitstream_pos += n;
    }
    pub fn write_bitstream(&mut self) {
        for v in &self.bitstream {
            self.main.extend_from_slice(&v.to_le_bytes());
        }
        self.position += (self.bitstream.len() * 4) as i32;
    }

    /// The frames, each padded to the longest.
    pub fn frame_data(&self) -> Vec<u8> {
        let n = self.frame_length as usize / 4;
        let mut out = Vec::with_capacity(self.frames.len() * n * 4);
        for f in &self.frames {
            for i in 0..n {
                out.extend_from_slice(&f.get(i).copied().unwrap_or(0).to_le_bytes());
            }
        }
        out
    }
}

/// `WriteToBitstream`: adds `bits` at bit `offset` (unmasked, and added rather
/// than or-ed, as the C# does).
fn to_bitstream(stream: &mut Vec<u32>, offset: i32, bits: u32, n: i32) {
    let soffset = offset % 32;
    let sindex = (offset / 32) as usize;
    while sindex >= stream.len() {
        stream.push(0);
    }
    stream[sindex] = stream[sindex].wrapping_add(bits.wrapping_shl(soffset as u32));
    let endbit = soffset + n - 32;
    if endbit > 0 {
        let eindex = sindex + 1;
        while eindex >= stream.len() {
            stream.push(0);
        }
        stream[eindex] = stream[eindex].wrapping_add(bits.wrapping_shr((32 - soffset) as u32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_round_trip() {
        let mut s = Vec::new();
        let mut pos = 0;
        let vals = [(5u32, 3), (0x1FFFF, 17), (1, 1), (0xABCDEF, 24), (0xFFFF_FFFF, 32), (0, 2)];
        for (v, n) in vals {
            to_bitstream(&mut s, pos, v, n);
            pos += n;
        }
        let data: Vec<u8> = s.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut r = SeqReader::new(&data, 0, 10, 0, 0);
        for (v, n) in vals {
            assert_eq!(r.read_bits(n), v);
        }
    }
}
