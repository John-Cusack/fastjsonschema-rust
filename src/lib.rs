use fastjsonschema_core::{engine, Ctxt, Plan};
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass]
struct Validator {
    plan: Plan,
    cx: Ctxt,
}

#[pymethods]
impl Validator {
    #[new]
    fn new(plan_dict: &Bound<PyDict>) -> PyResult<Self> {
        let plan = Plan::parse(plan_dict)?;
        Python::with_gil(|_py| {
            let py = _py;
            let exc_mod = py.import("fastjsonschema.exceptions")?;
            let decimal_mod = py.import("decimal")?;
            let re_mod = py.import("re")?;
            let base64_mod = py.import("base64")?;
            let json_mod = py.import("json")?;
            let cx = Ctxt {
                value_exc: exc_mod.getattr("JsonSchemaValueException")?.unbind(),
                values_exc: exc_mod.getattr("JsonSchemaValuesException")?.unbind(),
                decimal_cls: decimal_mod.getattr("Decimal")?.unbind(),
                re_mod: re_mod.unbind().into(),
                base64_mod: base64_mod.unbind().into(),
                json_mod: json_mod.unbind().into(),
                detailed: plan.detailed,
            };
            Ok(Self { plan, cx })
        })
    }

    #[pyo3(signature = (value, custom_formats=None, name_prefix=None))]
    fn validate<'py>(
        &self,
        value: &Bound<'py, PyAny>,
        custom_formats: Option<&Bound<'py, PyAny>>,
        name_prefix: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let py = value.py();
        let owned;
        let cfref: &Bound<'py, PyAny> = match custom_formats {
            Some(cf) => cf,
            None => {
                owned = PyDict::new(py).into_any();
                &owned
            }
        };
        engine::validate_one(py, &self.plan, &self.cx, value, cfref, name_prefix.unwrap_or(""))
    }

    #[pyo3(signature = (docs, custom_formats=None))]
    fn validate_many<'py>(
        &self,
        docs: &Bound<'py, PyAny>,
        custom_formats: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<usize> {
        let py = docs.py();
        let owned;
        let cfref: &Bound<'py, PyAny> = match custom_formats {
            Some(cf) => cf,
            None => {
                owned = PyDict::new(py).into_any();
                &owned
            }
        };
        let mut ok = 0usize;
        for doc in docs.try_iter()? {
            engine::validate_one(py, &self.plan, &self.cx, &doc?, cfref, "")?;
            ok += 1;
        }
        Ok(ok)
    }
}

#[pymodule]
fn _core(m: &Bound<PyModule>) -> PyResult<()> {
    m.add_class::<Validator>()?;
    Ok(())
}
