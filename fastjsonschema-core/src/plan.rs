//! Plan IR for the fastjsonschema-rust validation kernel.
//!
//! The plan is built in Python (`fastjsonschema_rust/_plan.py`) and parsed
//! here into an owned tree. All semantic values (thresholds, defaults,
//! definitions, enum members) stay as `Py<PyAny>` so comparisons, string
//! forms, and numeric behavior go through the C API with identical
//! Python semantics. Only structure is mirrored in Rust.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TypeName {
    Null,
    Boolean,
    Number,
    Integer,
    Str,
    Array,
    Object,
}

impl TypeName {
    fn parse(s: &str) -> Option<TypeName> {
        Some(match s {
            "null" => TypeName::Null,
            "boolean" => TypeName::Boolean,
            "number" => TypeName::Number,
            "integer" => TypeName::Integer,
            "string" => TypeName::Str,
            "array" => TypeName::Array,
            "object" => TypeName::Object,
            _ => return None,
        })
    }
}

pub enum Dynamic {
    None,
    Missing,
    ExtraSet,
    OneOf,
}

pub struct ErrTpl {
    pub before: String,
    pub after: String,
    pub rule: Option<String>,
    pub dynamic: Dynamic,
}

pub enum DepEntry {
    Fail { key: String, tpl: ErrTpl },
    Missing { key: String, value: Py<PyAny>, tpl: ErrTpl },
    Schema { key: String, branch: usize },
}

pub struct Prop {
    pub key: String,
    pub branch: usize,
    pub default: Option<Py<PyAny>>,
}

pub enum Additional {
    False,
    Branch(usize),
}

pub enum Check {
    True,
    False(ErrTpl),
    Type { types: Vec<TypeName>, draft06plus: bool, tpl: ErrTpl },
    Enum { values: Vec<Py<PyAny>>, tpl: ErrTpl },
    AllOf { branches: Vec<usize> },
    AnyOf { branches: Vec<usize>, tpl: ErrTpl },
    OneOf { branches: Vec<usize>, tpl: ErrTpl },
    Not { branch: usize, tpl: ErrTpl },
    MinLength { n: Py<PyAny>, tpl: ErrTpl },
    MaxLength { n: Py<PyAny>, tpl: ErrTpl },
    Pattern { rx: usize, tpl: ErrTpl },
    FormatRx { rx: usize, tpl: ErrTpl },
    FormatCall { name: String, tpl: ErrTpl },
    FormatRegex { tpl: ErrTpl },
    Minimum { n: Py<PyAny>, excl04: bool, tpl: ErrTpl },
    Maximum { n: Py<PyAny>, excl04: bool, tpl: ErrTpl },
    MultipleOf { n: Py<PyAny>, tpl: ErrTpl },
    ExclMinimum { n: Py<PyAny>, tpl: ErrTpl },
    ExclMaximum { n: Py<PyAny>, tpl: ErrTpl },
    MinItems { n: Py<PyAny>, tpl: ErrTpl },
    MaxItems { n: Py<PyAny>, tpl: ErrTpl },
    UniqueItems { tpl: ErrTpl },
    ItemsFalse { tpl: ErrTpl },
    ItemsEach { branch: usize },
    ItemsTuple { branches: Vec<usize>, defaults: Vec<Option<Py<PyAny>>>, additional: Option<Additional>, tpl: ErrTpl },
    MinProperties { n: Py<PyAny>, tpl: ErrTpl },
    MaxProperties { n: Py<PyAny>, tpl: ErrTpl },
    Required { keys: Vec<Py<PyAny>>, tpl: ErrTpl },
    Dependencies { entries: Vec<DepEntry>, empty: bool },
    Properties { props: Vec<Prop> },
    PatternProps { entries: Vec<(usize, usize)> },
    AddlFalse { props_keys: Vec<String>, tpl: ErrTpl },
    AddlSchema { branch: usize, props_keys: Vec<Py<PyAny>> },
    PropNamesFalse { tpl: ErrTpl },
    PropNames { branch: usize, tpl: ErrTpl },
    ContainsFalse { tpl: ErrTpl },
    ContainsTrue { tpl: ErrTpl },
    Contains { branch: usize, tpl: ErrTpl },
    Const { value: Py<PyAny>, tpl: ErrTpl },
    If { cond: usize, then: Option<usize>, els: Option<usize> },
    ContentBase64 { tpl: ErrTpl },
    ContentJsonBytes { tpl: ErrTpl },
    ContentJsonStr { tpl: ErrTpl },
    Ref { target: usize, pinned_fast: bool },
}

pub struct Level {
    pub checks: Vec<Check>,
    pub definition: Py<PyAny>,
}

pub struct Plan {
    pub levels: Vec<Level>,
    pub regexps: Vec<Py<PyAny>>,
    pub calls: Vec<Py<PyAny>>,
    pub use_default: bool,
    pub use_formats: bool,
    pub detailed: bool,
    pub fast_fail: bool,
}

fn get_str(d: &Bound<PyDict>, key: &str) -> PyResult<String> {
    d.get_item(key)?
        .unwrap_or_else(|| panic!("plan: missing {key}"))
        .extract::<String>()
        .map_err(|_| pyo3::exceptions::PyValueError::new_err(format!("plan: bad {key}")))
}

fn get_opt_str(d: &Bound<PyDict>, key: &str) -> PyResult<Option<String>> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract::<String>()?)),
        _ => Ok(None),
    }
}

fn get_bool(d: &Bound<PyDict>, key: &str) -> PyResult<bool> {
    d.get_item(key)?
        .unwrap_or_else(|| panic!("plan: missing {key}"))
        .extract::<bool>()
}

fn get_idx(d: &Bound<PyDict>, key: &str) -> PyResult<usize> {
    d.get_item(key)?
        .unwrap_or_else(|| panic!("plan: missing {key}"))
        .extract::<usize>()
}

fn get_opt_idx(d: &Bound<PyDict>, key: &str) -> PyResult<Option<usize>> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract::<usize>()?)),
        _ => Ok(None),
    }
}

fn get_py(d: &Bound<PyDict>, key: &str) -> PyResult<Py<PyAny>> {
    Ok(d
        .get_item(key)?
        .unwrap_or_else(|| panic!("plan: missing {key}"))
        .unbind())
}

fn get_opt_py(d: &Bound<PyDict>, key: &str) -> PyResult<Option<Py<PyAny>>> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.unbind())),
        _ => Ok(None),
    }
}

fn parse_tpl(d: &Bound<PyDict>) -> PyResult<(ErrTpl, ())> {
    let e: (String, String) = match d.get_item("e")? {
        Some(v) => {
            let mut parts: Vec<String> = v.extract()?;
            if parts.len() != 2 {
                return Err(pyo3::exceptions::PyValueError::new_err("plan: bad e"));
            }
            let after = parts.pop().unwrap();
            let before = parts.pop().unwrap();
            (before, after)
        }
        None => (String::new(), String::new()),
    };
    let rule = get_opt_str(d, "r")?;
    let dynamic = match get_opt_str(d, "dyn")?.as_deref() {
        Some("missing") => Dynamic::Missing,
        Some("extraset") => Dynamic::ExtraSet,
        Some("oneof") => Dynamic::OneOf,
        _ => Dynamic::None,
    };
    Ok((ErrTpl { before: e.0, after: e.1, rule, dynamic }, ()))
}

fn parse_check(d: &Bound<PyDict>) -> PyResult<Check> {
    let k = get_str(d, "k")?;
    let tpl = |d: &Bound<PyDict>| parse_tpl(d).map(|(t, _)| t);
    Ok(match k.as_str() {
        "true" => Check::True,
        "false" => Check::False(tpl(d)?),
        "type" => {
            let raw: Vec<String> = d.get_item("types")?.unwrap().extract()?;
            let types = raw.iter().map(|s| TypeName::parse(s).unwrap()).collect();
            let draft = get_str(d, "draft")?;
            Check::Type { types, draft06plus: draft != "04", tpl: tpl(d)? }
        }
        "enum" => {
            let vals: Vec<Py<PyAny>> = d.get_item("values")?.unwrap().extract()?;
            Check::Enum { values: vals, tpl: tpl(d)? }
        }
        "allof" => {
            let b: Vec<usize> = d.get_item("branches")?.unwrap().extract()?;
            Check::AllOf { branches: b }
        }
        "anyof" => {
            let b: Vec<usize> = d.get_item("branches")?.unwrap().extract()?;
            Check::AnyOf { branches: b, tpl: tpl(d)? }
        }
        "oneof" => {
            let b: Vec<usize> = d.get_item("branches")?.unwrap().extract()?;
            Check::OneOf { branches: b, tpl: tpl(d)? }
        }
        "not" => Check::Not { branch: get_idx(d, "branch")?, tpl: tpl(d)? },
        "minlength" => Check::MinLength { n: get_py(d, "n")?, tpl: tpl(d)? },
        "maxlength" => Check::MaxLength { n: get_py(d, "n")?, tpl: tpl(d)? },
        "pattern" => Check::Pattern { rx: get_idx(d, "rx")?, tpl: tpl(d)? },
        "format_rx" => Check::FormatRx { rx: get_idx(d, "rx")?, tpl: tpl(d)? },
        "format_call" => Check::FormatCall { name: get_str(d, "fmt")?, tpl: tpl(d)? },
        "format_regex" => Check::FormatRegex { tpl: tpl(d)? },
        "minimum" => Check::Minimum { n: get_py(d, "n")?, excl04: get_bool(d, "excl04")?, tpl: tpl(d)? },
        "maximum" => Check::Maximum { n: get_py(d, "n")?, excl04: get_bool(d, "excl04")?, tpl: tpl(d)? },
        "multipleof" => Check::MultipleOf { n: get_py(d, "n")?, tpl: tpl(d)? },
        "exclusiveminimum" => Check::ExclMinimum { n: get_py(d, "n")?, tpl: tpl(d)? },
        "exclusivemaximum" => Check::ExclMaximum { n: get_py(d, "n")?, tpl: tpl(d)? },
        "minitems" => Check::MinItems { n: get_py(d, "n")?, tpl: tpl(d)? },
        "maxitems" => Check::MaxItems { n: get_py(d, "n")?, tpl: tpl(d)? },
        "uniqueitems" => Check::UniqueItems { tpl: tpl(d)? },
        "items_false" => Check::ItemsFalse { tpl: tpl(d)? },
        "items_each" => Check::ItemsEach { branch: get_idx(d, "branch")? },
        "items_tuple" => {
            let branches: Vec<usize> = d.get_item("branches")?.unwrap().extract()?;
            let raw: Vec<Option<Py<PyAny>>> = d.get_item("defaults")?.unwrap().extract()?;
            let additional = match d.get_item("additional")? {
                Some(v) if v.is_none() => None,
                Some(v) => {
                    // Bool first: pyo3 extracts False as usize 0.
                    if let Ok(b) = v.extract::<bool>() {
                        if !b {
                            Some(Additional::False)
                        } else {
                            None
                        }
                    } else if let Ok(b) = v.extract::<usize>() {
                        Some(Additional::Branch(b))
                    } else {
                        None
                    }
                }
                None => None,
            };
            Check::ItemsTuple { branches, defaults: raw, additional, tpl: tpl(d)? }
        }
        "minproperties" => Check::MinProperties { n: get_py(d, "n")?, tpl: tpl(d)? },
        "maxproperties" => Check::MaxProperties { n: get_py(d, "n")?, tpl: tpl(d)? },
        "required" => {
            let keys: Vec<Py<PyAny>> = d.get_item("keys")?.unwrap().extract()?;
            Check::Required { keys, tpl: tpl(d)? }
        }
        "dependencies" => {
            let empty = get_bool(d, "empty")?;
            let mut entries = Vec::new();
            // entries serialized as list of [kind, key, payload]
            if let Some(list) = d.get_item("entries")? {
                let list = list.downcast::<PyList>().map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
                for item in list.iter() {
                    let t: (String, String, Py<PyAny>) = item.extract()?;
                    // payload shape disambiguated by kind at build time
                    entries.push(parse_dep(&t.0, &t.1, t.2)?);
                }
            }
            Check::Dependencies { entries, empty }
        }
        "properties" => {
            let mut props = Vec::new();
            let list = d.get_item("props")?.unwrap().downcast_into::<PyList>()?;
            for item in list.iter() {
                let pd = item.downcast::<PyDict>().map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
                props.push(Prop {
                    key: get_str(pd, "key")?,
                    branch: get_idx(pd, "branch")?,
                    default: get_opt_py(pd, "default")?,
                });
            }
            Check::Properties { props }
        }
        "patternproperties" => {
            let mut entries = Vec::new();
            let list = d.get_item("entries")?.unwrap().downcast_into::<PyList>()?;
            for item in list.iter() {
                let ed = item.downcast::<PyDict>().map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
                entries.push((get_idx(ed, "rx")?, get_idx(ed, "branch")?));
            }
            Check::PatternProps { entries }
        }
        "additional_true" => Check::True,
        "additional_false" => {
            let keys: Vec<String> = d.get_item("props_keys")?.unwrap().extract()?;
            Check::AddlFalse { props_keys: keys, tpl: tpl(d)? }
        }
        "additional_schema" => {
            let keys: Vec<Py<PyAny>> = d.get_item("props_keys")?.unwrap().extract()?;
            Check::AddlSchema { branch: get_idx(d, "branch")?, props_keys: keys }
        }
        "propertynames_false" => Check::PropNamesFalse { tpl: tpl(d)? },
        "propertynames" => Check::PropNames { branch: get_idx(d, "branch")?, tpl: tpl(d)? },
        "contains_false" => Check::ContainsFalse { tpl: tpl(d)? },
        "contains_true" => Check::ContainsTrue { tpl: tpl(d)? },
        "contains" => Check::Contains { branch: get_idx(d, "branch")?, tpl: tpl(d)? },
        "const" => Check::Const { value: get_py(d, "value")?, tpl: tpl(d)? },
        "ifthenelse" => Check::If {
            cond: get_idx(d, "cond")?,
            then: get_opt_idx(d, "then")?,
            els: get_opt_idx(d, "else")?,
        },
        "contentencoding_base64" => Check::ContentBase64 { tpl: tpl(d)? },
        "contentmediatype_json_bytes" => Check::ContentJsonBytes { tpl: tpl(d)? },
        "contentmediatype_json_str" => Check::ContentJsonStr { tpl: tpl(d)? },
        "ref" => Check::Ref { target: get_idx(d, "target")?, pinned_fast: get_bool(d, "pinned_fast")? },
        other => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "plan: unknown check kind {other}"
            )))
        }
    })
}

fn parse_dep(kind: &str, key: &str, payload: Py<PyAny>) -> PyResult<DepEntry> {
    // Payloads are passed through Python for template-bearing entries:
    // ('fail', key, tpl_dict) / ('missing', key, (value, tpl_dict)) /
    // ('schema', key, branch_idx). Re-serialize via pickle-free protocol:
    // the translator must emit matching shapes (see _plan wire format).
    Python::with_gil(|py| {
        let bound = payload.bind(py);
        match kind {
            "fail" => {
                let d = bound.downcast::<PyDict>()?;
                let (t, _) = parse_tpl(d)?;
                Ok(DepEntry::Fail { key: key.to_string(), tpl: t })
            }
            "missing" => {
                let tup = bound.downcast::<pyo3::types::PyTuple>()?;
                let value: Py<PyAny> = tup.get_item(0)?.unbind();
                let d = tup.get_item(1)?.downcast_into::<PyDict>()?;
                let (t, _) = parse_tpl(&d)?;
                Ok(DepEntry::Missing { key: key.to_string(), value, tpl: t })
            }
            _ => {
                let branch: usize = bound.extract()?;
                Ok(DepEntry::Schema { key: key.to_string(), branch })
            }
        }
    })
}

impl Plan {
    pub fn parse(plan: &Bound<PyDict>) -> PyResult<Plan> {
        let levels_raw = plan.get_item("levels")?.unwrap().downcast_into::<PyList>()?;
        let mut levels = Vec::with_capacity(levels_raw.len());
        for lvl in levels_raw.iter() {
            let ld = lvl.downcast::<PyDict>().map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let checks_raw = ld.get_item("checks")?.unwrap().downcast_into::<PyList>()?;
            let mut checks = Vec::with_capacity(checks_raw.len());
            for c in checks_raw.iter() {
                checks.push(parse_check(c.downcast::<PyDict>().map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?)?);
            }
            let definition = ld.get_item("d")?.unwrap().unbind();
            levels.push(Level { checks, definition });
        }
        let get_list = |key: &str| -> PyResult<Vec<Py<PyAny>>> {
            match plan.get_item(key)? {
                Some(v) => Ok(v.extract()?),
                None => Ok(Vec::new()),
            }
        };
        let opts = plan.get_item("options")?.unwrap().downcast_into::<PyDict>()?;
        Ok(Plan {
            levels,
            regexps: get_list("regexps")?,
            calls: get_list("calls")?,
            use_default: get_bool(&opts, "use_default")?,
            use_formats: get_bool(&opts, "use_formats")?,
            detailed: get_bool(&opts, "detailed")?,
            fast_fail: get_bool(&opts, "fast_fail")?,
        })
    }
}
