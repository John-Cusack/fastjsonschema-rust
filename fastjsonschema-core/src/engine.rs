//! Validation engine: executes a check plan against live Python values.
//!
//! All value-touching semantics (numeric tower, Decimal dance, set
//! ordering, regex, message building) go through the C ABI on the live
//! objects, so behavior matches upstream exactly. The win is removing
//! interpreter dispatch around those operations, not reimplementing them.

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PySet, PyString};

use crate::plan::{Additional, Check, DepEntry, Level, Plan, TypeName};

const MAX_DEPTH: usize = 4000;

pub struct Ctxt {
    pub value_exc: Py<PyAny>,
    pub values_exc: Py<PyAny>,
    pub decimal_cls: Py<PyAny>,
    pub re_mod: Py<PyAny>,
    pub base64_mod: Py<PyAny>,
    pub json_mod: Py<PyAny>,
    pub detailed: bool,
}

pub enum NameSeg {
    Key(String),
    Idx(usize),
}

pub enum DynErr {
    None,
    Missing(Vec<Py<PyAny>>),
    ExtraSet(Py<PyAny>),
    OneOf(usize),
}

pub struct RtError {
    pub before: String,
    pub name: String,
    pub after: String,
    pub dynamic: DynErr,
    pub rule: Option<String>,
    pub value: Py<PyAny>,
    pub definition: Py<PyAny>,
}

/// Error-path only: moved solely when validation fails, never on the
/// hot Ok path, so the large variant costs nothing per validated value.
#[allow(clippy::result_large_err)]
pub enum Fail {
    Plan(RtError),
    Raw(PyErr),
    Depth,
}

impl Fail {
    fn raw<T, E: Into<PyErr>>(r: Result<T, E>) -> Result<T, Fail> {
        r.map_err(|e| Fail::Raw(e.into()))
    }
    fn expect_raw(self) -> PyErr {
        match self {
            Fail::Raw(e) => e,
            _ => PyException::new_err("internal engine error"),
        }
    }
    fn down<T>(r: Result<T, pyo3::DowncastError<'_, '_>>) -> Result<T, Fail> {
        r.map_err(|e| Fail::Raw(e.into()))
    }
}

fn is_bool(_py: Python<'_>, v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<pyo3::types::PyBool>()
}

fn is_long(v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<pyo3::types::PyInt>()
}

fn is_float(v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<pyo3::types::PyFloat>()
}

fn is_decimal(py: Python<'_>, cx: &Ctxt, v: &Bound<'_, PyAny>) -> bool {
    v.is_instance(cx.decimal_cls.bind(py)).unwrap_or(false)
}

fn is_numeric(py: Python<'_>, cx: &Ctxt, v: &Bound<'_, PyAny>) -> bool {
    is_long(v) || is_float(v) || is_decimal(py, cx, v)
}

fn is_str(v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<PyString>()
}

fn is_dict(v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<PyDict>()
}

fn is_list(v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<PyList>()
}

fn is_tuple(v: &Bound<'_, PyAny>) -> bool {
    v.is_instance_of::<pyo3::types::PyTuple>()
}

fn is_listish(v: &Bound<'_, PyAny>) -> bool {
    is_list(v) || is_tuple(v)
}

fn is_integral_float(v: &Bound<'_, PyAny>) -> bool {
    if let Ok(f) = v.extract::<f64>() {
        f.fract() == 0.0
    } else {
        false
    }
}

fn type_match(py: Python<'_>, cx: &Ctxt, t: &TypeName, v: &Bound<'_, PyAny>, draft06plus: bool) -> bool {
    match t {
        TypeName::Null => v.is_none(),
        TypeName::Boolean => is_bool(py, v),
        TypeName::Number => is_numeric(py, cx, v),
        TypeName::Integer => {
            if is_long(v) {
                true
            } else {
                draft06plus && is_float(v) && is_integral_float(v)
            }
        }
        TypeName::Str => is_str(v),
        TypeName::Array => is_listish(v),
        TypeName::Object => is_dict(v),
    }
}

/// Structural matcher mirroring upstream `_enum_value_matches`.
fn enum_match(py: Python<'_>, pat: &Bound<'_, PyAny>, v: &Bound<'_, PyAny>) -> PyResult<bool> {
    if pat.is_instance_of::<pyo3::types::PyBool>() {
        return Ok(is_bool(py, v) && v.eq(pat)?);
    }
    if is_float(pat) || (is_long(pat) && !pat.is_instance_of::<pyo3::types::PyBool>()) {
        if !(is_long(v) || is_float(v)) || is_bool(py, v) {
            return Ok(false);
        }
        return v.eq(pat);
    }
    if pat.is_none() {
        return Ok(v.is_none());
    }
    if is_str(pat) {
        return Ok(is_str(v) && v.eq(pat)?);
    }
    if is_dict(pat) {
        if !is_dict(v) {
            return Ok(false);
        }
        let pd = Fail::down(pat.downcast::<PyDict>()).map_err(|f| match f {
            Fail::Raw(e) => e,
            _ => unreachable!(),
        })?;
        let vd = Fail::down(v.downcast::<PyDict>()).map_err(|f| match f {
            Fail::Raw(e) => e,
            _ => unreachable!(),
        })?;
        if pd.len() != vd.len() {
            return Ok(false);
        }
        for (k, pv) in pd.iter() {
            match vd.get_item(&k)? {
                Some(vv) => {
                    if !enum_match(py, &pv, &vv)? {
                        return Ok(false);
                    }
                }
                None => return Ok(false),
            }
        }
        return Ok(true);
    }
    if is_list(pat) || is_tuple(pat) {
        if !is_listish(v) {
            return Ok(false);
        }
        let n = pat.len()?;
        if n != v.len()? {
            return Ok(false);
        }
        for i in 0..n {
            let pv = pat.get_item(i)?;
            let vv = v.get_item(i)?;
            if !enum_match(py, &pv, &vv)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    v.eq(pat)
}

fn truthy_call(callable: &Bound<'_, PyAny>, arg: &Bound<'_, PyAny>) -> Result<bool, Fail> {
    let r = Fail::raw(callable.call1((arg,)))?;
    Fail::raw(r.is_truthy())
}

fn search_hit(rx: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>, method: &str) -> Result<bool, Fail> {
    let r = Fail::raw(rx.call_method1(method, (value,)))?;
    Fail::raw(r.is_truthy())
}

/// Normalizer for uniqueItems, mirroring upstream `fn`.
fn unq_norm<'py>(py: Python<'py>, v: &Bound<'py, PyAny>) -> Result<Py<PyAny>, Fail> {
    if Fail::raw(v.hasattr("items"))? {
        let items = Fail::raw(v.call_method0("items"))?;
        let mut pairs = Vec::new();
        for item in Fail::raw(items.try_iter())? {
            let item = Fail::raw(item)?;
            let pair = Fail::down(item.downcast::<pyo3::types::PyTuple>())?;
            let k = Fail::raw(pair.get_item(0))?;
            let val = Fail::raw(pair.get_item(1))?;
            let nv = unq_norm(py, &val)?;
            let t = pyo3::types::PyTuple::new(py, [&k, nv.bind(py)]).map_err(Fail::Raw)?;
            pairs.push(t.into_any().unbind());
        }
        let pl = PyList::new(py, pairs).map_err(Fail::Raw)?;
        let fs = py.get_type::<pyo3::types::PyFrozenSet>();
        return Fail::raw(fs.call1((pl,))).map(|b| b.unbind());
    }
    if is_listish(v) {
        let n = Fail::raw(v.len())?;
        let mut items = Vec::with_capacity(n);
        for i in 0..n {
            items.push(unq_norm(py, &Fail::raw(v.get_item(i))?)?);
        }
        let t = pyo3::types::PyTuple::new(py, items).map_err(Fail::Raw)?;
        return Ok(t.into_any().unbind());
    }
    if v.is_instance_of::<pyo3::types::PyBool>() {
        return Fail::raw(v.str()).map(|b| b.into_any().unbind());
    }
    Ok(v.clone().unbind())
}

fn dyn_text(py: Python<'_>, dyn_: &DynErr) -> Result<String, Fail> {
    match dyn_ {
        DynErr::None => Ok(String::new()),
        DynErr::Missing(keys) => {
            let list = PyList::new(py, keys.iter().map(|k| k.bind(py))).map_err(Fail::Raw)?;
            Fail::raw(list.sort())?;
            let st = Fail::raw(list.str())?;
            Ok(format!("{} properties", Fail::raw(st.to_str())?))
        }
        DynErr::ExtraSet(set) => {
            let st = Fail::raw(set.bind(py).str())?;
            Ok(format!("{} properties", Fail::raw(st.to_str())?))
        }
        DynErr::OneOf(n) => Ok(format!(" ({n} matches found)")),
    }
}

fn build_message(py: Python<'_>, cx: &Ctxt, e: &RtError) -> Result<String, Fail> {
    // detailed=False drops append_to_msg tails (required missing-list,
    // oneOf count) but keeps message-embedded expressions
    // (additionalProperties set, evaluated at runtime like upstream).
    let tail = match &e.dynamic {
        DynErr::None => String::new(),
        DynErr::ExtraSet(_) => dyn_text(py, &e.dynamic)?,
        _ if cx.detailed => dyn_text(py, &e.dynamic)?,
        _ => String::new(),
    };
    let mut s = String::with_capacity(e.before.len() + e.name.len() + e.after.len() + tail.len());
    s.push_str(&e.before);
    s.push_str(&e.name);
    s.push_str(&e.after);
    s.push_str(&tail);
    Ok(s)
}

fn value_err_obj<'py>(py: Python<'py>, cx: &Ctxt, e: &RtError) -> Result<Bound<'py, PyAny>, Fail> {
    let message = build_message(py, cx, e)?;
    if cx.detailed {
        let rule: Py<PyAny> = match &e.rule {
            Some(r) => PyString::new(py, r).into_any().unbind(),
            None => py.None(),
        };
        Ok(Fail::raw(cx.value_exc.bind(py).call1((
            message,
            e.value.bind(py),
            e.name.clone(),
            e.definition.bind(py),
            rule.bind(py),
        )))?)
    } else {
        Ok(Fail::raw(cx.value_exc.bind(py).call1((message,)))?)
    }
}

fn plan_to_pyerr(py: Python<'_>, cx: &Ctxt, e: &RtError) -> Result<PyErr, Fail> {
    Ok(PyErr::from_value(value_err_obj(py, cx, e)?))
}

fn render_name(prefix: &str, segs: &[NameSeg]) -> String {
    let mut s = String::with_capacity(16 + segs.len() * 8);
    if prefix.is_empty() {
        s.push_str("data");
    } else {
        s.push_str(prefix);
    }
    for seg in segs {
        match seg {
            NameSeg::Key(k) => {
                s.push('.');
                s.push_str(k);
            }
            NameSeg::Idx(i) => {
                s.push('[');
                s.push_str(&i.to_string());
                s.push(']');
            }
        }
    }
    s
}

#[allow(clippy::too_many_arguments)]
fn check_level<'py>(
    py: Python<'py>,
    plan: &Plan,
    cx: &Ctxt,
    idx: usize,
    value: &Bound<'py, PyAny>,
    prefix: &str,
    segs: &mut Vec<NameSeg>,
    depth: usize,
    out: &mut Vec<RtError>,
    trial: bool,
    custom_formats: &Bound<'py, PyAny>,
) -> Result<Bound<'py, PyAny>, Fail> {
    if depth > MAX_DEPTH {
        return Err(Fail::Depth);
    }
    let level = &plan.levels[idx];
    let mut cur = value.clone();
    for chk in &level.checks {
        cur = apply_check(py, plan, cx, idx, chk, &cur, prefix, segs, depth, out, trial, custom_formats)?;
    }
    Ok(cur)
}

fn py_len(v: &Bound<'_, PyAny>) -> Result<usize, Fail> {
    Fail::raw(v.len())
}

#[allow(clippy::too_many_arguments)]
fn apply_check<'py>(
    py: Python<'py>,
    plan: &Plan,
    cx: &Ctxt,
    lvl_idx: usize,
    chk: &Check,
    value: &Bound<'py, PyAny>,
    prefix: &str,
    segs: &mut Vec<NameSeg>,
    depth: usize,
    out: &mut Vec<RtError>,
    trial: bool,
    custom_formats: &Bound<'py, PyAny>,
) -> Result<Bound<'py, PyAny>, Fail> {
    let collect = !trial && !plan.fast_fail;
    let level: &Level = &plan.levels[lvl_idx];
    let definition = level.definition.bind(py).clone();
    let fail_here = |prefix: &str,
                     segs: &[NameSeg],
                     tpl: &crate::plan::ErrTpl,
                     dyn_: DynErr,
                     v: &Bound<'py, PyAny>,
                     definition: &Bound<'py, PyAny>,
                     out: &mut Vec<RtError>|
     -> Result<(), Fail> {
        let e = RtError {
            before: tpl.before.clone(),
            name: render_name(prefix, segs),
            after: tpl.after.clone(),
            dynamic: dyn_,
            rule: tpl.rule.clone(),
            value: v.clone().unbind(),
            definition: definition.clone().unbind(),
        };
        if collect {
            out.push(e);
            Ok(())
        } else {
            Err(Fail::Plan(e))
        }
    };
    match chk {
        Check::True => {}
        Check::False(tpl) => {
            fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
        }
        Check::Type { types, draft06plus, tpl } => {
            let hit = types.iter().any(|t| type_match(py, cx, t, value, *draft06plus));
            let bool_excl = (types.contains(&TypeName::Number)
                || types.contains(&TypeName::Integer))
                && !types.contains(&TypeName::Boolean)
                && is_bool(py, value);
            if !hit || bool_excl {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::Enum { values, tpl } => {
            let mut hit = false;
            for pat in values {
                if Fail::raw(enum_match(py, pat.bind(py), value))? {
                    hit = true;
                    break;
                }
            }
            if !hit {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::Const { value: pat, tpl } => {
            if !Fail::raw(enum_match(py, pat.bind(py), value))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::AllOf { branches } => {
            for b in branches {
                check_level(py, plan, cx, *b, value, prefix, segs, depth + 1, out, trial, custom_formats)?;
            }
        }
        Check::AnyOf { branches, tpl } => {
            let mut hit = false;
            for b in branches {
                if check_level(py, plan, cx, *b, value, prefix, segs, depth + 1, out, true, custom_formats)
                    .is_ok()
                {
                    hit = true;
                    break;
                }
            }
            if !hit {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::OneOf { branches, tpl } => {
            let mut count = 0usize;
            for b in branches {
                if count < 2
                    && check_level(py, plan, cx, *b, value, prefix, segs, depth + 1, out, true, custom_formats)
                        .is_ok()
                {
                    count += 1;
                }
            }
            if count != 1 {
                fail_here(prefix, segs, tpl, DynErr::OneOf(count), value, &definition, out)?;
            }
        }
        Check::Not { branch, tpl } => {
            if check_level(py, plan, cx, *branch, value, prefix, segs, depth + 1, out, true, custom_formats).is_ok() {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::MinLength { n, tpl } => {
            if is_str(value) && Fail::raw(lt_int(py, py_len(value)? as i64, n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::MaxLength { n, tpl } => {
            if is_str(value) && Fail::raw(gt_int(py, py_len(value)? as i64, n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::Pattern { rx, tpl } => {
            if is_str(value) {
                let rxo = plan.regexps[*rx].bind(py);
                if !search_hit(rxo, value, "search")? {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::FormatRx { rx, tpl } => {
            if is_str(value) {
                let rxo = plan.regexps[*rx].bind(py);
                if !search_hit(rxo, value, "match")? {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::FormatCall { name, tpl } => {
            if is_str(value) {
                let func: Bound<'py, PyAny> = Fail::raw(custom_formats.get_item(name.as_str()))?;
                if !truthy_call(&func, value)? {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::FormatRegex { tpl } => {
            if is_str(value) {
                match cx.re_mod.bind(py).call_method1("compile", (value,)) {
                    Ok(_) => {}
                    Err(e) => {
                        if e.is_instance_of::<PyException>(py) {
                            fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                        } else {
                            return Err(Fail::Raw(e));
                        }
                    }
                }
            }
        }
        Check::Minimum { n, excl04, tpl } => {
            if is_numeric(py, cx, value) && !is_bool(py, value) {
                let bad = if *excl04 {
                    Fail::raw(value.le(n.bind(py)))?
                } else {
                    Fail::raw(value.lt(n.bind(py)))?
                };
                if bad {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::Maximum { n, excl04, tpl } => {
            if is_numeric(py, cx, value) && !is_bool(py, value) {
                let bad = if *excl04 {
                    Fail::raw(value.ge(n.bind(py)))?
                } else {
                    Fail::raw(value.gt(n.bind(py)))?
                };
                if bad {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::ExclMinimum { n, tpl } => {
            if is_numeric(py, cx, value) && !is_bool(py, value) && Fail::raw(value.le(n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::ExclMaximum { n, tpl } => {
            if is_numeric(py, cx, value) && !is_bool(py, value) && Fail::raw(value.ge(n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::MultipleOf { n, tpl } => {
            if is_numeric(py, cx, value) && !is_bool(py, value) {
                let nb = n.bind(py);
                let quotient = if is_float(nb) {
                    let dec = cx.decimal_cls.bind(py);
                    let a = Fail::raw(dec.call1((Fail::raw(value.repr())?,)))?;
                    let b = Fail::raw(dec.call1((Fail::raw(nb.repr())?,)))?;
                    Fail::raw(a.div(&b))?
                } else {
                    Fail::raw(value.div(nb))?
                };
                let qi = Fail::raw(quotient.call_method0("__int__"))?;
                if Fail::raw(qi.ne(&quotient))? {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                } else {
                    let ratio = Fail::raw(value.div(nb))?;
                    let inf: Bound<'py, PyAny> =
                        f64::INFINITY.into_pyobject(py).expect("inf").into_any();
                    if Fail::raw(ratio.eq(&inf))? {
                        return Err(Fail::Plan(RtError {
                            before: "inifinity reached".to_string(),
                            name: String::new(),
                            after: String::new(),
                            dynamic: DynErr::None,
                            rule: tpl.rule.clone(),
                            value: value.clone().unbind(),
                            definition: definition.clone().unbind(),
                        }));
                    }
                }
            }
        }
        Check::MinItems { n, tpl } => {
            if is_listish(value) && Fail::raw(lt_int(py, py_len(value)? as i64, n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::MaxItems { n, tpl } => {
            if is_listish(value) && Fail::raw(gt_int(py, py_len(value)? as i64, n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::UniqueItems { tpl } => {
            if is_listish(value) {
                let n = py_len(value)?;
                let set = PySet::empty(py).map_err(Fail::Raw)?;
                for i in 0..n {
                    let item = Fail::raw(value.get_item(i))?;
                    Fail::raw(set.add(unq_norm(py, &item)?))?;
                }
                if n > set.len() {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::ItemsFalse { tpl } => {
            if is_listish(value) && py_len(value)? > 0 {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::ItemsEach { branch } => {
            if is_listish(value) {
                let n = py_len(value)?;
                for i in 0..n {
                    let item = Fail::raw(value.get_item(i))?;
                    segs.push(NameSeg::Idx(i));
                    let r = check_level(
                        py, plan, cx, *branch, &item, prefix, segs, depth + 1, out, trial, custom_formats,
                    );
                    segs.pop();
                    r?;
                }
            }
        }
        Check::ItemsTuple { branches, defaults, additional, tpl } => {
            if is_listish(value) {
                let n = py_len(value)?;
                for (i, b) in branches.iter().enumerate() {
                    if n > i {
                        let item = Fail::raw(value.get_item(i))?;
                        segs.push(NameSeg::Idx(i));
                        let r = check_level(
                            py, plan, cx, *b, &item, prefix, segs, depth + 1, out, trial, custom_formats,
                        );
                        segs.pop();
                        r?;
                    } else if plan.use_default {
                        if let Some(def) = defaults.get(i).and_then(|o| o.as_ref()) {
                            Fail::raw(value.call_method1("append", (def.bind(py),)))?;
                        }
                    }
                }
                match additional {
                    None => {}
                    Some(Additional::False) => {
                        if n > branches.len() {
                            fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                        }
                    }
                    Some(Additional::Branch(b)) => {
                        for i in branches.len()..n {
                            let item = Fail::raw(value.get_item(i))?;
                            segs.push(NameSeg::Idx(i));
                            let r = check_level(
                                py, plan, cx, *b, &item, prefix, segs, depth + 1, out, trial, custom_formats,
                            );
                            segs.pop();
                            r?;
                        }
                    }
                }
            }
        }
        Check::MinProperties { n, tpl } => {
            if is_dict(value) && Fail::raw(lt_int(py, py_len(value)? as i64, n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::MaxProperties { n, tpl } => {
            if is_dict(value) && Fail::raw(gt_int(py, py_len(value)? as i64, n.bind(py)))? {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::Required { keys, tpl } => {
            if is_dict(value) {
                let d = Fail::down(value.downcast::<PyDict>())?;
                let mut missing = Vec::new();
                for k in keys {
                    if !Fail::raw(d.contains(k.bind(py)))? {
                        missing.push(k.clone_ref(py));
                    }
                }
                if !missing.is_empty() {
                    fail_here(prefix, segs, tpl, DynErr::Missing(missing), value, &definition, out)?;
                }
            }
        }
        Check::Dependencies { entries, empty } => {
            if !empty && is_dict(value) {
                let d = Fail::down(value.downcast::<PyDict>())?;
                for e in entries {
                    match e {
                        DepEntry::Fail { key, tpl } => {
                            if Fail::raw(d.contains(PyString::new(py, key)))? {
                                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                            }
                        }
                        DepEntry::Missing { key, value: dep, tpl } => {
                            if Fail::raw(d.contains(PyString::new(py, key)))?
                                && !Fail::raw(d.contains(dep.bind(py)))?
                            {
                                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                            }
                        }
                        DepEntry::Schema { key, branch } => {
                            if Fail::raw(d.contains(PyString::new(py, key)))? {
                                check_level(
                                    py, plan, cx, *branch, value, prefix, segs, depth + 1, out, trial,
                                    custom_formats,
                                )?;
                            }
                        }
                    }
                }
            }
        }
        Check::Properties { props } => {
            if is_dict(value) {
                let d = Fail::down(value.downcast::<PyDict>())?;
                for p in props {
                    match Fail::raw(d.get_item(PyString::new(py, &p.key)))? {
                        Some(item) => {
                            segs.push(NameSeg::Key(p.key.clone()));
                            let r = check_level(
                                py, plan, cx, p.branch, &item, prefix, segs, depth + 1, out, trial,
                                custom_formats,
                            );
                            segs.pop();
                            r?;
                        }
                        None => {
                            if let Some(def) = p.default.as_ref() {
                                if plan.use_default {
                                    Fail::raw(d.set_item(PyString::new(py, &p.key), def.bind(py)))?;
                                }
                            }
                        }
                    }
                }
            }
        }
        Check::PatternProps { entries } => {
            if is_dict(value) {
                let d = Fail::down(value.downcast::<PyDict>())?;
                for (rx, branch) in entries {
                    let rxo = plan.regexps[*rx].bind(py);
                    let items: Vec<(Py<PyAny>, Py<PyAny>)> = Fail::raw(
                        d.iter()
                            .map(|(k, v)| Ok::<_, PyErr>((k.unbind(), v.unbind())))
                            .collect::<PyResult<_>>(),
                    )?;
                    for (k, v) in items {
                        let kb = k.bind(py);
                        if is_str(kb) && search_hit(rxo, kb, "search")? {
                            let kss = Fail::raw(kb.str())?.to_string_lossy().into_owned();
                            segs.push(NameSeg::Key(kss));
                            let r = check_level(
                                py, plan, cx, *branch, v.bind(py), prefix, segs, depth + 1, out, trial,
                                custom_formats,
                            );
                            segs.pop();
                            r?;
                        }
                    }
                }
            }
        }
        Check::AddlFalse { tpl, .. } => {
            if is_dict(value) {
                let set = keyset_for(py, plan, lvl_idx, value)?;
                if set.len() > 0 {
                    fail_here(
                        prefix,
                        segs,
                        tpl,
                        DynErr::ExtraSet(set.into_any().unbind()),
                        value,
                        &definition,
                        out,
                    )?;
                }
            }
        }
        Check::AddlSchema { branch, .. } => {
            if is_dict(value) {
                let set = keyset_for(py, plan, lvl_idx, value)?;
                for key in set.iter() {
                    let kss = Fail::raw(key.str())?.to_string_lossy().into_owned();
                    segs.push(NameSeg::Key(kss));
                    let item = Fail::raw(value.get_item(&key))?;
                    let r = check_level(
                        py, plan, cx, *branch, &item, prefix, segs, depth + 1, out, trial,
                        custom_formats,
                    );
                    segs.pop();
                    r?;
                }
            }
        }
        Check::PropNamesFalse { tpl } => {
            if is_dict(value) && py_len(value)? > 0 {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::PropNames { branch, tpl } => {
            if is_dict(value) && py_len(value)? != 0 {
                let d = Fail::down(value.downcast::<PyDict>())?;
                let mut ok = true;
                for k in d.keys() {
                    if check_level(py, plan, cx, *branch, &k, prefix, segs, depth + 1, out, true, custom_formats)
                        .is_err()
                    {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::ContainsFalse { tpl } => {
            if is_listish(value) {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::ContainsTrue { tpl } => {
            if is_listish(value) && py_len(value)? == 0 {
                fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
            }
        }
        Check::Contains { branch, tpl } => {
            if is_listish(value) {
                let n = py_len(value)?;
                let mut hit = false;
                for i in 0..n {
                    let item = Fail::raw(value.get_item(i))?;
                    if check_level(
                        py, plan, cx, *branch, &item, prefix, segs, depth + 1, out, true, custom_formats,
                    )
                    .is_ok()
                    {
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                }
            }
        }
        Check::If { cond, then, els } => {
            let passed =
                check_level(py, plan, cx, *cond, value, prefix, segs, depth + 1, out, true, custom_formats).is_ok();
            if passed {
                if let Some(t) = then {
                    check_level(py, plan, cx, *t, value, prefix, segs, depth + 1, out, trial, custom_formats)?;
                }
            } else if let Some(e) = els {
                check_level(py, plan, cx, *e, value, prefix, segs, depth + 1, out, trial, custom_formats)?;
            }
        }
        Check::ContentBase64 { tpl } => {
            if is_str(value) {
                match cx.base64_mod.bind(py).call_method1("b64decode", (value,)) {
                    Ok(decoded) => return Ok(decoded),
                    Err(e) => {
                        if e.is_instance_of::<PyException>(py) {
                            fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                        } else {
                            return Err(Fail::Raw(e));
                        }
                    }
                }
            }
        }
        Check::ContentJsonBytes { tpl } => {
            if value.is_instance_of::<pyo3::types::PyBytes>() {
                match value.call_method1("decode", ("utf-8",)) {
                    Ok(decoded) => return Ok(decoded),
                    Err(e) => {
                        if e.is_instance_of::<PyException>(py) {
                            fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                        } else {
                            return Err(Fail::Raw(e));
                        }
                    }
                }
            }
        }
        Check::ContentJsonStr { tpl } => {
            if is_str(value) {
                match cx.json_mod.bind(py).call_method1("loads", (value,)) {
                    Ok(parsed) => return Ok(parsed),
                    Err(e) => {
                        if e.is_instance_of::<PyException>(py) {
                            fail_here(prefix, segs, tpl, DynErr::None, value, &definition, out)?;
                        } else {
                            return Err(Fail::Raw(e));
                        }
                    }
                }
            }
        }
        Check::Ref { target, pinned_fast } => {
            if collect && !pinned_fast {
                let mut sub: Vec<RtError> = Vec::new();
                match check_level(py, plan, cx, *target, value, prefix, segs, depth + 1, &mut sub, false, custom_formats)
                {
                    Ok(_) => out.extend(sub),
                    Err(Fail::Plan(e)) => {
                        out.extend(sub);
                        out.push(e);
                    }
                    Err(other) => return Err(other),
                }
            } else if collect && *pinned_fast {
                check_level(py, plan, cx, *target, value, prefix, segs, depth + 1, out, true, custom_formats)?;
            } else {
                check_level(py, plan, cx, *target, value, prefix, segs, depth + 1, out, trial, custom_formats)?;
            }
        }
    }
    Ok(value.clone())
}

fn lt_int(py: Python<'_>, a: i64, b: &Bound<'_, PyAny>) -> PyResult<bool> {
    let ao: Bound<'_, PyAny> = a.into_pyobject(py).expect("int").into_any();
    ao.lt(b)
}

fn gt_int(py: Python<'_>, a: i64, b: &Bound<'_, PyAny>) -> PyResult<bool> {
    let ao: Bound<'_, PyAny> = a.into_pyobject(py).expect("int").into_any();
    ao.gt(b)
}

/// Shared pruned key-set for a level (mirrors upstream's single _keys set).
fn keyset_for<'py>(
    py: Python<'py>,
    plan: &Plan,
    lvl_idx: usize,
    value: &Bound<'py, PyAny>,
) -> Result<Bound<'py, PySet>, Fail> {
    let d = Fail::down(value.downcast::<PyDict>())?;
    let set = PySet::empty(py).map_err(Fail::Raw)?;
    for k in d.keys() {
        Fail::raw(set.add(k))?;
    }
    for chk in &plan.levels[lvl_idx].checks {
        match chk {
            Check::Properties { props } => {
                for p in props {
                    let k = PyString::new(py, &p.key);
                    if Fail::raw(set.contains(&k))? {
                        Fail::raw(set.discard(&k))?;
                    }
                }
            }
            Check::PatternProps { entries } => {
                for (rx, _) in entries {
                    let rxo = plan.regexps[*rx].bind(py);
                    let keys: Vec<Py<PyAny>> =
                        Fail::raw(set.iter().map(|k| Ok::<_, PyErr>(k.unbind())).collect::<PyResult<_>>())?;
                    for k in keys {
                        let kb = k.bind(py);
                        if is_str(kb) && search_hit(rxo, kb, "search")? && Fail::raw(set.contains(kb))? {
                            Fail::raw(set.discard(kb))?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(set)
}

pub fn validate_one<'py>(
    py: Python<'py>,
    plan: &Plan,
    cx: &Ctxt,
    value: &Bound<'py, PyAny>,
    custom_formats: &Bound<'py, PyAny>,
    prefix: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let trial = plan.fast_fail;
    let mut out: Vec<RtError> = Vec::new();
    let mut segs: Vec<NameSeg> = Vec::new();
    let r = check_level(py, plan, cx, 0, value, prefix, &mut segs, 0, &mut out, trial, custom_formats);
    match r {
        Ok(final_value) => {
            if trial || out.is_empty() {
                Ok(final_value)
            } else {
                Err(values_err(py, cx, &out).map_err(Fail::expect_raw)?)
            }
        }
        Err(Fail::Plan(e)) => {
            if trial {
                Err(plan_to_pyerr(py, cx, &e).map_err(Fail::expect_raw)?)
            } else {
                out.push(e);
                Err(values_err(py, cx, &out).map_err(Fail::expect_raw)?)
            }
        }
        Err(Fail::Raw(e)) => Err(e),
        Err(Fail::Depth) => Err(pyo3::exceptions::PyRecursionError::new_err(
            "maximum recursion depth exceeded",
        )),
    }
}

fn values_err(py: Python<'_>, cx: &Ctxt, out: &[RtError]) -> Result<PyErr, Fail> {
    let mut objs = Vec::with_capacity(out.len());
    for e in out {
        objs.push(value_err_obj(py, cx, e)?);
    }
    let list = PyList::new(py, objs).map_err(Fail::Raw)?;
    let exc = Fail::raw(cx.values_exc.bind(py).call1((list,)))?;
    Ok(PyErr::from_value(exc))
}
