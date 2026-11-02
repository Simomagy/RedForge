//! Animations (`Rsc8Anim`): bone ids and sequences of packed channel data.

use bytemuck::{Pod, Zeroable};

use super::ReadCx;
use super::channel::{CHANNEL_TYPE_COUNT, Channel, ChannelData, ChannelRef, SeqReader, SeqWriter};
use super::util::{file_name_without_extension, net_sort, read_array, read_str, write_ptr_arr};
use crate::rsc8::{ArrayHeader, BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// Which bone (or other value) a track animates, and what it is: `type_id` 0 a
/// vector, 1 a quaternion, 2 a float; `track_id` 0 position, 1 rotation, 5 and
/// 6 root motion, ...
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct BoneId {
    pub bone_id: u16,
    pub type_id: u8,
    pub track_id: u8,
}

impl BoneId {
    /// `WriteStruct`: the fields that aren't 0.
    pub fn write_xml(&self, w: &mut XmlWriter) {
        if self.bone_id != 0 {
            w.u16("BoneId", self.bone_id);
        }
        if self.type_id != 0 {
            w.u8("TypeId", self.type_id);
        }
        if self.track_id != 0 {
            w.u8("TrackId", self.track_id);
        }
    }
    pub fn read_xml(n: &XmlNode) -> Self {
        Self { bone_id: n.u16("BoneId", 0), type_id: n.u8("TypeId", 0), track_id: n.u8("TrackId", 0) }
    }
}

/// `Rsc8Anim`, 0x70 bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    pub usage_count: u64,
    /// The owning dictionary's address, as read (not followed).
    pub owner: u64,
    pub flags: u16,
    pub dict_index: u16,
    pub frame_count: u16,
    pub sequence_size: u16,
    pub duration: f32,
    pub signature: u32,
    /// `None` only when it came from XML without one.
    pub name: Option<String>,
    pub unknown_38h: u64,
    pub unknown_40h: u64,
    pub max_seq_block_length: u64,
    pub sequences: Option<Vec<Option<Sequence>>>,
    pub bone_ids: Option<Vec<BoneId>>,
}

impl Default for Anim {
    fn default() -> Self {
        Self {
            unknown_00h: 0,
            unknown_08h: 0,
            usage_count: 0,
            owner: 0,
            flags: 0,
            dict_index: 0,
            frame_count: 0,
            sequence_size: 0,
            duration: 0.0,
            signature: 0,
            name: None,
            unknown_38h: 0,
            unknown_40h: 0,
            max_seq_block_length: 0,
            sequences: None,
            bone_ids: None,
        }
    }
}

impl Anim {
    /// Reads the animation at `ptr`.
    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let mut a = Anim {
                unknown_00h: r.u64()?,
                unknown_08h: r.u64()?,
                usage_count: r.u64()?,
                owner: r.u64()?,
                flags: r.u16()?,
                dict_index: r.u16()?,
                frame_count: r.u16()?,
                sequence_size: r.u16()?,
                duration: r.f32()?,
                signature: r.u32()?,
                ..Default::default()
            };
            let name = read_str(r)?;
            a.unknown_38h = r.u64()?;
            a.unknown_40h = r.u64()?;
            a.max_seq_block_length = r.u64()?;
            let seqs = ArrayHeader::read(r)?;
            let bones = ArrayHeader::read(r)?;
            if let Some(ptrs) = read_array::<u64>(r, seqs.ptr, seqs.count as usize)? {
                let mut v = Vec::with_capacity(ptrs.len());
                for p in ptrs {
                    v.push(if p == 0 { None } else { Some(Sequence::read(r, p)?) });
                }
                a.sequences = Some(v);
            }
            a.bone_ids = read_array::<BoneId>(r, bones.ptr, bones.count as usize)?;
            let name = name.unwrap_or_default();
            cx.hashes.ensure(file_name_without_extension(&name));
            a.name = Some(name);
            a.assign_sequence_bone_ids()?;
            Ok(a)
        })
    }

    /// Gives each sequence's tracks their bone ids (by index), failing like the
    /// C# when there's a gap in the tracks.
    pub fn assign_sequence_bone_ids(&mut self) -> Result<()> {
        let (Some(seqs), Some(ids)) = (self.sequences.as_mut(), self.bone_ids.as_ref()) else { return Ok(()) };
        for seq in seqs.iter_mut().flatten() {
            let Some(tracks) = seq.tracks.as_mut() else { continue };
            for (i, t) in tracks.iter_mut().enumerate() {
                if let Some(id) = ids.get(i) {
                    match t {
                        Some(t) => t.bone_id = *id,
                        None => bail!("animation sequence has no track {i} for its bone id"),
                    }
                }
            }
        }
        Ok(())
    }

    /// The signature the game's files have: a Fletcher-32 checksum of the bone
    /// ids, as two 16-bit words each.
    pub fn calc_signature(&self) -> u32 {
        let (mut a, mut b) = (0u32, 0u32);
        for id in self.bone_ids.as_deref().unwrap_or(&[]) {
            a = (a + (((id.track_id as u32) << 8) | id.type_id as u32) + 0x80) % 65535;
            b = (b + a) % 65535;
            a = (a + id.bone_id as u32) % 65535;
            b = (b + a) % 65535;
        }
        (b << 16) | a
    }

    /// Fails where the C# fails writing the XML (an indirect quantised value
    /// out of range).
    pub fn check_xml(&self) -> Result<()> {
        for seq in self.sequences.iter().flatten().flatten() {
            for t in seq.tracks.iter().flatten().flatten() {
                for c in t.channels.iter().flatten().flatten() {
                    c.frame_values()?;
                }
            }
        }
        Ok(())
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", self.name.as_deref().unwrap_or(""));
        w.u16("Flags", self.flags);
        w.u16("DictIndex", self.dict_index);
        w.u16("FrameCount", self.frame_count);
        w.u16("SequenceSize", self.sequence_size);
        w.f32("Duration", self.duration);
        w.u32("Signature", self.signature);
        w.node_array("BoneIds", self.bone_ids.as_deref(), |w, b| b.write_xml(w));
        w.node_array_opt("Sequences", self.sequences.as_deref(), |w, s| s.write_xml(w));
    }

    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let mut a = Anim {
            name: n.string("Name"),
            flags: n.u16("Flags", 0),
            dict_index: n.u16("DictIndex", 0),
            frame_count: n.u16("FrameCount", 0),
            sequence_size: n.u16("SequenceSize", 0),
            duration: n.f32("Duration", 0.0),
            signature: n.u32("Signature", 0),
            ..Default::default()
        };
        a.bone_ids = n.child("BoneIds").map(|b| b.children("Item").map(|i| BoneId::read_xml(&i)).collect());
        if let Some(items) = n.node_array("Sequences") {
            let mut v = Vec::with_capacity(items.len());
            for i in items {
                v.push(match i {
                    Some(i) => Some(Sequence::read_xml(&i)?),
                    None => None,
                });
            }
            a.sequences = Some(v);
        }
        if a.sequences.as_ref().is_some_and(|s| s.len() == 1) {
            // What every single-sequence animation in the game has, whatever the XML says.
            a.sequence_size = ((((a.frame_count as i32) + 15) & !15) - 1) as u16;
        }
        a.usage_count = 1;
        a.assign_sequence_bone_ids()?;
        Ok(a)
    }

    /// `BuildSequencesData` and the signature, as the C# does before writing:
    /// packs sequences that came from XML, and works out the sizes.
    pub fn build(&mut self) -> Result<()> {
        self.assign_sequence_bone_ids()?;
        let mut max = 0u32;
        if let Some(seqs) = self.sequences.as_mut() {
            for s in seqs.iter_mut() {
                let Some(s) = s else { bail!("animation with a null sequence") };
                s.pack()?;
            }
            for s in seqs.iter().flatten() {
                max = max.max(s.block_length() as u32);
            }
        }
        self.max_seq_block_length = max as u64;
        self.signature = self.calc_signature();
        Ok(())
    }

    /// Adds the animation's block (after [`Anim::build`]) at `id`.
    pub fn write(&self, w: &mut Rsc8Writer, id: BlockId, owner: Option<BlockId>) {
        let mut b = BlockBuf::with_capacity(0x70);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u64(self.usage_count);
        b.ptr(owner);
        b.u16(self.flags);
        b.u16(self.dict_index);
        b.u16(self.frame_count);
        b.u16(self.sequence_size);
        b.f32(self.duration);
        b.u32(self.signature);
        b.ptr(w.opt_string(self.name.as_deref()));
        b.u64(self.unknown_38h);
        b.u64(self.unknown_40h);
        b.u64(self.max_seq_block_length);
        let seqs: Option<Vec<Option<BlockId>>> =
            self.sequences.as_ref().map(|v| v.iter().map(|s| s.as_ref().map(|s| s.write(w))).collect());
        write_ptr_arr(w, &mut b, seqs.as_deref());
        let n = self.bone_ids.as_ref().map_or(0, |v| v.len()) as u16;
        b.ptr(w.array(self.bone_ids.as_deref()));
        b.u16(n);
        b.u16(n);
        b.u32(0);
        w.set(id, b, false);
    }
}

/// A track: the channels of one bone id, by index (x, y, z, w, or a cached
/// quaternion at 3 or 4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub channel_count: i32,
    pub channels: Option<Vec<Option<Channel>>>,
    pub bone_id: BoneId,
}

impl Track {
    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.node_array_opt("Channels", self.channels.as_deref(), |w, c| c.write_xml(w));
    }
    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let mut t = Track::default();
        if let Some(items) = n.node_array("Channels") {
            let mut v = Vec::with_capacity(items.len());
            for i in items {
                v.push(match i {
                    Some(i) => Some(Channel::read_xml(i.attr("type"), &i)?),
                    None => None,
                });
            }
            t.channels = Some(v);
        }
        Ok(t)
    }
}

/// `Rsc8AnimSequence`: a run of frames of an animation, as packed data
/// (`data`) and unpacked into tracks of channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub signature: u32,
    pub data_length: u32,
    pub unknown_08h: u32,
    pub frame_offset: u32,
    pub root_motion_refs_offset: u32,
    pub unknown_14h: u16,
    pub frame_count: u16,
    pub frame_length: u16,
    pub indirect_quantize_float_num_ints: u16,
    pub quantize_float_value_bits: u16,
    pub chunk_size: u8,
    pub root_motion_ref_counts: u8,
    /// The packed data. Sequences read from a file keep it and are written back
    /// as they were; those from XML get it from [`Sequence::pack`].
    pub data: Option<Vec<u8>>,
    pub tracks: Option<Vec<Option<Track>>>,
    pub root_position_refs: Option<Vec<ChannelRef>>,
    pub root_rotation_refs: Option<Vec<ChannelRef>>,
}

impl Default for Sequence {
    fn default() -> Self {
        Self {
            signature: 0,
            data_length: 0,
            unknown_08h: 0,
            frame_offset: 0,
            root_motion_refs_offset: 0,
            unknown_14h: 1,
            frame_count: 0,
            frame_length: 0,
            indirect_quantize_float_num_ints: 0,
            quantize_float_value_bits: 0,
            chunk_size: 10,
            root_motion_ref_counts: 0,
            data: None,
            tracks: None,
            root_position_refs: None,
            root_rotation_refs: None,
        }
    }
}

impl Sequence {
    pub fn block_length(&self) -> u64 {
        0x20 + self.data_length as u64
    }

    pub fn read(r: &mut Rsc8Reader, ptr: u64) -> Result<Self> {
        r.at(ptr, |r| {
            let mut s = Sequence {
                signature: r.u32()?,
                data_length: r.u32()?,
                unknown_08h: r.u32()?,
                frame_offset: r.u32()?,
                root_motion_refs_offset: r.u32()?,
                unknown_14h: r.u16()?,
                frame_count: r.u16()?,
                frame_length: r.u16()?,
                indirect_quantize_float_num_ints: r.u16()?,
                quantize_float_value_bits: r.u16()?,
                chunk_size: r.u8()?,
                root_motion_ref_counts: r.u8()?,
                ..Default::default()
            };
            s.data = Some(r.bytes(s.data_length as usize)?.to_vec());
            s.unpack()?;
            Ok(s)
        })
    }

    /// Reads the tracks and root motion references out of the packed data.
    pub fn unpack(&mut self) -> Result<()> {
        let data = self.data.as_deref().unwrap_or(&[]);
        let mut r = SeqReader::new(data, self.frame_count, self.chunk_size, self.frame_offset, self.frame_length);
        let mut lists: Vec<Vec<Channel>> = vec![Vec::new(); CHANNEL_TYPE_COUNT];
        let mut frame_offset = 0i32;
        let mut track_max = 0i32;
        for (ty, list) in lists.iter_mut().enumerate() {
            let count = r.read_channel_count()? as usize;
            if count == 0 {
                continue;
            }
            for _ in 0..count {
                let mut ch = Channel::new(ty as i64).unwrap();
                let bits = r.read_channel_data_bits()?;
                ch.data_offset = (r.position / 4) as i32;
                ch.frame_offset = frame_offset;
                ch.read_header(&mut r)?;
                ch.track = (bits >> 2) as i32;
                ch.index = (bits & 3) as i32;
                if let ChannelData::CachedQuaternion { second, quat_index } = &mut ch.data {
                    *quat_index = ch.index;
                    ch.index = if *second { 4 } else { 3 };
                }
                frame_offset = frame_offset.wrapping_add(ch.frame_bits);
                track_max = track_max.max(ch.track);
                list.push(ch);
            }
            r.align_channel_data_offset(count);
        }

        for f in 0..self.frame_count as i32 {
            r.begin_frame(f);
            for ch in lists.iter_mut().flatten() {
                ch.read_frame(&mut r);
            }
        }

        let mut tracks: Vec<Option<Track>> = vec![None; track_max as usize + 1];
        for ch in lists.iter().flatten() {
            let t = tracks[ch.track as usize].get_or_insert_with(Track::default);
            t.channel_count = t.channel_count.max(ch.index + 1);
        }
        for ch in lists.into_iter().flatten() {
            let t = tracks[ch.track as usize].as_mut().unwrap();
            let n = t.channel_count as usize;
            let i = ch.index as usize;
            t.channels.get_or_insert_with(|| vec![None; n])[i] = Some(ch);
        }
        self.tracks = Some(tracks);

        let pos = ((self.root_motion_ref_counts >> 4) & 0xF) as usize;
        let rot = (self.root_motion_ref_counts & 0xF) as usize;
        if pos > 0 {
            self.root_position_refs = Some((0..pos).map(|_| r.read_channel_ref()).collect::<Result<_>>()?);
        }
        if rot > 0 {
            self.root_rotation_refs = Some((0..rot).map(|_| r.read_channel_ref()).collect::<Result<_>>()?);
        }
        Ok(())
    }

    /// Packs the tracks into data, as the C# does for sequences from XML (it
    /// does nothing when there is data already): channels by type, frames of
    /// quantised bits, the root motion references, and the signature.
    pub fn pack(&mut self) -> Result<()> {
        if self.data.is_some() {
            return Ok(());
        }
        let Some(tracks) = self.tracks.as_mut() else { return Ok(()) };
        let mut w = SeqWriter::new(self.frame_count);
        let mut lists: Vec<Vec<(usize, usize)>> = vec![Vec::new(); CHANNEL_TYPE_COUNT];
        for (t, track) in tracks.iter_mut().enumerate() {
            let Some(chans) = track.as_mut().and_then(|t| t.channels.as_mut()) else { continue };
            for (c, chan) in chans.iter_mut().enumerate() {
                let Some(chan) = chan else { continue };
                if let ChannelData::IncrementalFloat { values, .. } = &mut chan.data {
                    // There's no encoder for these: their values go raw.
                    let mut raw = Channel::new(3).unwrap();
                    raw.data = ChannelData::RawFloat { values: values.take() };
                    raw.track = chan.track;
                    raw.index = chan.index;
                    *chan = raw;
                }
                if let ChannelData::CachedQuaternion { quat_index, .. } = chan.data {
                    chan.index = quat_index;
                }
                lists[chan.type_id()].push((t, c));
            }
        }

        let track_count = tracks.len();
        for list in &lists {
            let count = list.len() as u16 as usize;
            w.channel_list_data(count as u16);
            for &at in &list[..count] {
                let ch = chan_at(tracks, at);
                w.channel_item_data((ch.index + (ch.track << 2)) as u16);
                ch.data_offset = w.position / 4;
                ch.write_header(&mut w)?;
            }
            w.align_channel_item_data(count, track_count);
        }

        for f in 0..self.frame_count as i32 {
            w.begin_frame(f);
            for list in &lists {
                let count = list.len() as u16 as usize;
                for &at in &list[..count] {
                    chan_at(tracks, at).write_frame(&mut w)?;
                }
            }
            w.end_frame();
        }

        let mut frame_offset = 0i32;
        for &at in lists.iter().flatten() {
            let ch = chan_at(tracks, at);
            ch.frame_offset = frame_offset;
            frame_offset = frame_offset.wrapping_add(ch.frame_bits);
        }

        self.update_root_motion_refs();
        for r in self.root_position_refs.iter().chain(self.root_rotation_refs.iter()).flatten() {
            w.channel_ref(r);
        }

        let mut data = w.main.clone();
        data.extend_from_slice(&w.frame_data());
        data.extend_from_slice(&w.channel_list);
        data.extend_from_slice(&w.channel_items);

        self.signature = Self::calc_signature(&data);
        self.data_length = data.len() as u32;
        self.frame_offset = w.main.len() as u32;
        self.frame_length = w.frame_length;
        self.chunk_size = 10;
        let tracks = self.tracks.as_deref().unwrap_or(&[]);
        let channels = || tracks.iter().flatten().filter_map(|t| t.channels.as_ref()).flatten().flatten();
        self.quantize_float_value_bits = channels()
            .filter(|c| matches!(c.data, ChannelData::QuantizeFloat { .. }))
            .fold(0i32, |a, c| a.wrapping_add(c.frame_bits)) as u16;
        self.indirect_quantize_float_num_ints = channels()
            .filter_map(|c| match c.data {
                ChannelData::IndirectQuantizeFloat { num_ints, .. } => Some(num_ints + 5),
                _ => None,
            })
            .fold(0i32, |a, n| a.wrapping_add(n)) as u16;
        let pos = self.root_position_refs.as_ref().map_or(0, |v| v.len()) as u32;
        let rot = self.root_rotation_refs.as_ref().map_or(0, |v| v.len()) as u32;
        self.root_motion_ref_counts = ((pos << 4) | rot) as u8;
        let refs = ((self.root_motion_ref_counts >> 4) & 0xF) as u64 + (self.root_motion_ref_counts & 0xF) as u64;
        self.root_motion_refs_offset = (0x20 + data.len() as u64 - refs * 6) as u32;
        self.data = Some(data);
        Ok(())
    }

    /// The signature the game's files have: the Jenkins one-at-a-time hash of the
    /// packed data, its bytes taken as signed.
    pub fn calc_signature(data: &[u8]) -> u32 {
        let mut h: u32 = 0;
        for &b in data {
            h = h.wrapping_add(b as i8 as i32 as u32);
            h = h.wrapping_add(h << 10);
            h ^= h >> 6;
        }
        h = h.wrapping_add(h << 3);
        h ^= h >> 11;
        h.wrapping_add(h << 15)
    }

    /// The references to the channels of the root position (track id 5) and
    /// rotation (6) tracks, sorted by channel type and index.
    fn update_root_motion_refs(&mut self) {
        let mut pos = Vec::new();
        let mut rot = Vec::new();
        for t in self.tracks.iter().flatten().flatten() {
            let list = match t.bone_id.track_id {
                5 => &mut pos,
                6 => &mut rot,
                _ => continue,
            };
            for c in t.channels.iter().flatten().flatten() {
                list.push(ChannelRef {
                    channel_type: c.type_id() as u8,
                    channel_index: c.index as u8,
                    data_int_offset: c.data_offset as u16,
                    frame_bit_offset: c.frame_offset as u16,
                });
            }
        }
        let cmp = |a: &ChannelRef, b: &ChannelRef| {
            a.channel_type.cmp(&b.channel_type).then(a.channel_index.cmp(&b.channel_index))
        };
        net_sort(&mut pos, cmp);
        net_sort(&mut rot, cmp);
        self.root_position_refs = (!pos.is_empty()).then_some(pos);
        self.root_rotation_refs = (!rot.is_empty()).then_some(rot);
    }

    /// Adds the sequence's block (header and data).
    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let data = self.data.as_deref().unwrap_or(&[]);
        let mut b = BlockBuf::with_capacity(0x20 + data.len());
        b.u32(self.signature);
        b.u32(self.data_length);
        b.u32(self.unknown_08h);
        b.u32(self.frame_offset);
        b.u32(self.root_motion_refs_offset);
        b.u16(self.unknown_14h);
        b.u16(self.frame_count);
        b.u16(self.frame_length);
        b.u16(self.indirect_quantize_float_num_ints);
        b.u16(self.quantize_float_value_bits);
        b.u8(self.chunk_size);
        b.u8(self.root_motion_ref_counts);
        b.bytes(data);
        b.pad_to(self.block_length() as usize);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u32("Signature", self.signature);
        w.u16("FrameCount", self.frame_count);
        w.node_array_opt("Tracks", self.tracks.as_deref(), |w, t| t.write_xml(w));
    }

    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let mut s =
            Sequence { signature: n.u32("Signature", 0), frame_count: n.u16("FrameCount", 0), ..Default::default() };
        if let Some(items) = n.node_array("Tracks") {
            let mut v = Vec::with_capacity(items.len());
            for i in items {
                v.push(match i {
                    Some(i) => Some(Track::read_xml(&i)?),
                    None => None,
                });
            }
            // Each channel's track and index are where it is.
            for (t, track) in v.iter_mut().enumerate() {
                for (c, ch) in track.iter_mut().filter_map(|t| t.channels.as_mut()).flatten().enumerate() {
                    if let Some(ch) = ch {
                        ch.track = t as i32;
                        ch.index = c as i32;
                    }
                }
            }
            s.tracks = Some(v);
        }
        Ok(s)
    }
}

/// The channel at a (track, index) position of the packing lists.
fn chan_at(tracks: &mut [Option<Track>], (t, c): (usize, usize)) -> &mut Channel {
    tracks[t].as_mut().unwrap().channels.as_mut().unwrap()[c].as_mut().unwrap()
}
