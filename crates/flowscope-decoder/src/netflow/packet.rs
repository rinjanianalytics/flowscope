// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! NetFlow v5, NetFlow v9 and IPFIX packet parsing.
//!
//! This follows the behavior of goflow2 (used by the Go version) closely,
//! including how it handles truncated records and padding, as the decoded
//! flows must be the same:
//!
//! - a data set using an unknown template is skipped, the remaining sets are
//!   still decoded;
//! - any other error drops the whole packet;
//! - trailing bytes smaller than a record are ignored.

use serde::{Deserialize, Serialize};

/// Parse error.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// A data set references a template we do not know. Not fatal.
    TemplateNotFound,
    /// Anything else. The packet is dropped.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TemplateNotFound => f.write_str("template not found"),
            Error::Invalid(s) => f.write_str(s),
        }
    }
}

fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}

/// A cursor over a buffer, like Go's bytes.Buffer: `next()` returns at most
/// the requested number of bytes.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn next(&mut self, n: usize) -> &'a [u8] {
        let n = n.min(self.data.len());
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        head
    }

    fn exact(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let b = self.next(n);
        if b.len() < n { Err(invalid("unexpected EOF")) } else { Ok(b) }
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.exact(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        let b = self.exact(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        let b = self.exact(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Go encodes empty slices as null.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Field>, D::Error> {
    Ok(Option::<Vec<Field>>::deserialize(d)?.unwrap_or_default())
}

/// A field of a template.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    #[serde(rename = "pen-provided")]
    pub pen_provided: bool,
    #[serde(rename = "type")]
    pub r#type: u16,
    pub length: u16,
    pub pen: u32,
}

/// A data template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRecord {
    #[serde(rename = "template-id")]
    pub template_id: u16,
    #[serde(rename = "field-count")]
    pub field_count: u16,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub fields: Vec<Field>,
}

/// A NetFlow v9 options template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nfv9OptionsTemplateRecord {
    #[serde(rename = "template-id")]
    pub template_id: u16,
    #[serde(rename = "scope-length")]
    pub scope_length: u16,
    #[serde(rename = "option-length")]
    pub option_length: u16,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub scopes: Vec<Field>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub options: Vec<Field>,
}

/// An IPFIX options template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpfixOptionsTemplateRecord {
    #[serde(rename = "template-id")]
    pub template_id: u16,
    #[serde(rename = "field-count")]
    pub field_count: u16,
    #[serde(rename = "scope-field-count")]
    pub scope_field_count: u16,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub options: Vec<Field>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub scopes: Vec<Field>,
}

/// A template of any kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Template {
    Data(TemplateRecord),
    Nfv9Options(Nfv9OptionsTemplateRecord),
    IpfixOptions(IpfixOptionsTemplateRecord),
}

impl Template {
    /// Label used in metrics.
    pub fn kind(&self) -> &'static str {
        match self {
            Template::Data(_) => "template",
            _ => "options_template",
        }
    }
}

/// Where templates are stored and looked up while parsing.
pub trait TemplateStore {
    fn get_template(&self, version: u16, obs_domain_id: u32, template_id: u16) -> Option<Template>;
    fn add_template(&self, version: u16, obs_domain_id: u32, template_id: u16, template: Template);
}

/// A decoded field value. `value` is `None` when the record was too short
/// to decode the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataField<'a> {
    pub pen_provided: bool,
    pub r#type: u16,
    pub pen: u32,
    pub value: Option<&'a [u8]>,
}

/// An options data record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionsDataRecord<'a> {
    pub scopes: Vec<DataField<'a>>,
    pub options: Vec<DataField<'a>>,
}

/// A flow set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowSet<'a> {
    Template(Vec<TemplateRecord>),
    Nfv9OptionsTemplate(Vec<Nfv9OptionsTemplateRecord>),
    IpfixOptionsTemplate(Vec<IpfixOptionsTemplateRecord>),
    Data(Vec<Vec<DataField<'a>>>),
    OptionsData(Vec<OptionsDataRecord<'a>>),
    /// A data set whose template is unknown.
    Raw,
}

/// Header of a NetFlow v9 or IPFIX packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    /// NetFlow v9: number of records. IPFIX: length.
    pub count_or_length: u16,
    /// NetFlow v9 only.
    pub system_uptime: u32,
    /// NetFlow v9: unix seconds. IPFIX: export time.
    pub unix_seconds: u32,
    pub sequence_number: u32,
    /// NetFlow v9: source ID. IPFIX: observation domain ID.
    pub obs_domain_id: u32,
}

/// A parsed NetFlow v9 or IPFIX packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub header: Header,
    pub flow_sets: Vec<FlowSet<'a>>,
}

fn decode_field(r: &mut Reader, pen: bool) -> Result<Field, Error> {
    let mut f = Field { r#type: r.u16()?, length: r.u16()?, ..Default::default() };
    // In options templates, the enterprise bit is kept in the type.
    if pen && f.r#type & 0x8000 != 0 {
        f.pen_provided = true;
        f.pen = r.u32()?;
    }
    Ok(f)
}

fn decode_template_set(version: u16, mut r: Reader) -> Result<Vec<TemplateRecord>, Error> {
    let mut records = Vec::new();
    while r.len() >= 4 {
        let template_id = r.u16()?;
        let field_count = r.u16()?;
        let mut fields = Vec::with_capacity(field_count as usize);
        for _ in 0..field_count {
            let mut f = Field {
                r#type: r.u16().map_err(|_| invalid("TemplateSet: reading field"))?,
                length: r.u16().map_err(|_| invalid("TemplateSet: reading field"))?,
                ..Default::default()
            };
            if version == 10 && f.r#type & 0x8000 != 0 {
                f.pen_provided = true;
                f.r#type ^= 0x8000;
                f.pen = r.u32().map_err(|_| invalid("TemplateSet: reading enterprise field"))?;
            }
            fields.push(f);
        }
        records.push(TemplateRecord { template_id, field_count, fields });
    }
    Ok(records)
}

fn decode_nfv9_options_template_set(mut r: Reader) -> Result<Vec<Nfv9OptionsTemplateRecord>, Error> {
    let mut records = Vec::new();
    while r.len() >= 4 {
        let template_id = r.u16()?;
        let scope_length = r.u16()?;
        let option_length = r.u16().map_err(|_| invalid("NFv9OptionsTemplateSet: header"))?;
        let scopes = (0..scope_length / 4).map(|_| decode_field(&mut r, false)).collect::<Result<_, _>>()?;
        let options = (0..option_length / 4).map(|_| decode_field(&mut r, false)).collect::<Result<_, _>>()?;
        records.push(Nfv9OptionsTemplateRecord { template_id, scope_length, option_length, scopes, options });
    }
    Ok(records)
}

fn decode_ipfix_options_template_set(mut r: Reader) -> Result<Vec<IpfixOptionsTemplateRecord>, Error> {
    let mut records = Vec::new();
    while r.len() >= 4 {
        let template_id = r.u16()?;
        let field_count = r.u16()?;
        let scope_field_count = r.u16().map_err(|_| invalid("IPFIXOptionsTemplateSet: header"))?;
        let scopes = (0..scope_field_count).map(|_| decode_field(&mut r, true)).collect::<Result<_, _>>()?;
        if field_count < scope_field_count {
            return Err(invalid("IPFIXOptionsTemplateSet: negative length"));
        }
        let options =
            (0..field_count - scope_field_count).map(|_| decode_field(&mut r, true)).collect::<Result<_, _>>()?;
        records.push(IpfixOptionsTemplateRecord { template_id, field_count, scope_field_count, options, scopes });
    }
    Ok(records)
}

/// Sum of the fixed lengths of a template.
fn template_size(fields: &[Field]) -> usize {
    fields.iter().filter(|f| f.length != 0xffff).map(|f| f.length as usize).sum()
}

fn decode_values<'a>(r: &mut Reader<'a>, fields: &[Field]) -> Result<Vec<DataField<'a>>, Error> {
    let mut out: Vec<DataField> =
        fields.iter().map(|_| DataField { pen_provided: false, r#type: 0, pen: 0, value: None }).collect();
    if r.len() < template_size(fields) {
        return Ok(out);
    }
    for (i, f) in fields.iter().enumerate() {
        let mut length = f.length as usize;
        if f.length == 0xffff {
            let l8 = r.u8().map_err(|_| invalid("DataSet: variable length header"))?;
            length = if l8 == 0xff {
                r.u16().map_err(|_| invalid("DataSet: extended variable length"))? as usize
            } else {
                l8 as usize
            };
        }
        out[i] = DataField { pen_provided: f.pen_provided, r#type: f.r#type, pen: f.pen, value: Some(r.next(length)) };
    }
    Ok(out)
}

fn decode_data_set<'a>(mut r: Reader<'a>, fields: &[Field]) -> Result<Vec<Vec<DataField<'a>>>, Error> {
    let size = template_size(fields);
    let mut records = Vec::new();
    while r.len() > 0 && r.len() >= size {
        let before = r.len();
        let values = decode_values(&mut r, fields)?;
        if r.len() == before {
            return Err(invalid("DataSet: record of zero size"));
        }
        records.push(values);
    }
    Ok(records)
}

fn decode_options_data_set<'a>(
    mut r: Reader<'a>,
    scopes: &[Field],
    options: &[Field],
) -> Result<Vec<OptionsDataRecord<'a>>, Error> {
    let size = template_size(scopes) + template_size(options);
    let mut records = Vec::new();
    while r.len() > 0 && r.len() >= size {
        let before = r.len();
        let scopes = decode_values(&mut r, scopes)?;
        let options = decode_values(&mut r, options)?;
        if r.len() == before {
            return Err(invalid("OptionsDataSet: record of zero size"));
        }
        records.push(OptionsDataRecord { scopes, options });
    }
    Ok(records)
}

fn decode_flow_set<'a>(
    r: &mut Reader<'a>,
    store: &dyn TemplateStore,
    obs_domain_id: u32,
    version: u16,
) -> Result<FlowSet<'a>, Error> {
    let id = r.u16().map_err(|_| invalid("flow set header"))?;
    let length = r.u16().map_err(|_| invalid("flow set header"))?;
    if length < 4 {
        return Err(invalid("negative length"));
    }
    let body = Reader::new(r.next(length as usize - 4));
    match (id, version) {
        (0, 9) | (2, 10) => {
            let records = decode_template_set(version, body)?;
            for t in &records {
                store.add_template(version, obs_domain_id, t.template_id, Template::Data(t.clone()));
            }
            Ok(FlowSet::Template(records))
        }
        (1, 9) => {
            let records = decode_nfv9_options_template_set(body)?;
            for t in &records {
                store.add_template(version, obs_domain_id, t.template_id, Template::Nfv9Options(t.clone()));
            }
            Ok(FlowSet::Nfv9OptionsTemplate(records))
        }
        (3, 10) => {
            let records = decode_ipfix_options_template_set(body)?;
            for t in &records {
                store.add_template(version, obs_domain_id, t.template_id, Template::IpfixOptions(t.clone()));
            }
            Ok(FlowSet::IpfixOptionsTemplate(records))
        }
        (id, _) if id >= 256 => match store.get_template(version, obs_domain_id, id) {
            None => Err(Error::TemplateNotFound),
            Some(Template::Data(t)) => Ok(FlowSet::Data(decode_data_set(body, &t.fields)?)),
            Some(Template::Nfv9Options(t)) => {
                Ok(FlowSet::OptionsData(decode_options_data_set(body, &t.scopes, &t.options)?))
            }
            Some(Template::IpfixOptions(t)) => {
                Ok(FlowSet::OptionsData(decode_options_data_set(body, &t.scopes, &t.options)?))
            }
        },
        _ => Err(invalid(format!("flow set ID error ({id})"))),
    }
}

/// Parse a NetFlow v9 or IPFIX packet (including the version). Returns the
/// packet and whether some data sets used an unknown template. A fatal error
/// drops the packet.
pub fn parse<'a>(payload: &'a [u8], store: &dyn TemplateStore) -> Result<(Packet<'a>, bool), Error> {
    let mut r = Reader::new(payload);
    let version = r.u16().map_err(|_| invalid("version"))?;
    let header = match version {
        9 => Header {
            version,
            count_or_length: r.u16()?,
            system_uptime: r.u32()?,
            unix_seconds: r.u32()?,
            sequence_number: r.u32()?,
            obs_domain_id: r.u32().map_err(|_| invalid("NetFlowV9 header"))?,
        },
        10 => Header {
            version,
            count_or_length: r.u16()?,
            unix_seconds: r.u32()?,
            sequence_number: r.u32()?,
            obs_domain_id: r.u32().map_err(|_| invalid("IPFIX header"))?,
            system_uptime: 0,
        },
        v => return Err(invalid(format!("unknown version {v}"))),
    };
    // For IPFIX, stop after the announced length (minus the header).
    let size = header.count_or_length.wrapping_sub(16) as usize;
    let start = r.len();
    let mut flow_sets = Vec::new();
    let mut missing_template = false;
    while r.len() >= 4 && (version == 9 || start - r.len() < size) {
        match decode_flow_set(&mut r, store, header.obs_domain_id, version) {
            Ok(fs) => flow_sets.push(fs),
            Err(Error::TemplateNotFound) => {
                missing_template = true;
                flow_sets.push(FlowSet::Raw);
            }
            Err(e) => return Err(e),
        }
    }
    Ok((Packet { header, flow_sets }, missing_template))
}

/// A NetFlow v5 record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct V5Record {
    pub src_addr: u32,
    pub dst_addr: u32,
    pub next_hop: u32,
    pub input: u16,
    pub output: u16,
    pub d_pkts: u32,
    pub d_octets: u32,
    pub first: u32,
    pub last: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub tcp_flags: u8,
    pub proto: u8,
    pub tos: u8,
    pub src_as: u16,
    pub dst_as: u16,
    pub src_mask: u8,
    pub dst_mask: u8,
}

/// A NetFlow v5 packet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct V5Packet {
    pub count: u16,
    pub sys_uptime: u32,
    pub unix_secs: u32,
    pub unix_nsecs: u32,
    pub flow_sequence: u32,
    pub engine_type: u8,
    pub engine_id: u8,
    pub sampling_interval: u16,
    pub records: Vec<V5Record>,
}

/// Parse a NetFlow v5 packet, without the version. Only the records present
/// are returned (goflow2 returns `count` records, with zero ones when the
/// packet is shorter).
pub fn parse_v5(payload: &[u8]) -> Result<V5Packet, Error> {
    let mut r = Reader::new(payload);
    let header_err = |_| invalid("NetFlow v5 header");
    let mut p = V5Packet {
        count: r.u16().map_err(header_err)?,
        sys_uptime: r.u32().map_err(header_err)?,
        unix_secs: r.u32().map_err(header_err)?,
        unix_nsecs: r.u32().map_err(header_err)?,
        flow_sequence: r.u32().map_err(header_err)?,
        engine_type: r.u8().map_err(header_err)?,
        engine_id: r.u8().map_err(header_err)?,
        sampling_interval: r.u16().map_err(header_err)?,
        records: Vec::new(),
    };
    p.records = Vec::with_capacity(p.count.min(30) as usize);
    for _ in 0..p.count as usize {
        if r.len() < 48 {
            break;
        }
        p.records.push(V5Record::default());
        let rec = p.records.last_mut().unwrap();
        rec.src_addr = r.u32()?;
        rec.dst_addr = r.u32()?;
        rec.next_hop = r.u32()?;
        rec.input = r.u16()?;
        rec.output = r.u16()?;
        rec.d_pkts = r.u32()?;
        rec.d_octets = r.u32()?;
        rec.first = r.u32()?;
        rec.last = r.u32()?;
        rec.src_port = r.u16()?;
        rec.dst_port = r.u16()?;
        r.u8()?; // pad
        rec.tcp_flags = r.u8()?;
        rec.proto = r.u8()?;
        rec.tos = r.u8()?;
        rec.src_as = r.u16()?;
        rec.dst_as = r.u16()?;
        rec.src_mask = r.u8()?;
        rec.dst_mask = r.u8()?;
        r.u16()?; // pad
    }
    Ok(p)
}
