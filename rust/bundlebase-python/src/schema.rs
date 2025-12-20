use arrow::datatypes::SchemaRef;
use arrow_schema::FieldRef;
use pyo3::prelude::*;

#[pyclass]
#[derive(Clone)]
pub struct PySchemaField {
    inner: FieldRef,
}

#[pymethods]
impl PySchemaField {
    #[getter]
    fn name(&self) -> String {
        self.inner.name().clone()
    }

    #[getter]
    fn nullable(&self) -> bool {
        self.inner.is_nullable()
    }

    #[getter]
    fn data_type(&self) -> String {
        self.inner.data_type().to_string()
    }

    fn __repr__(&self) -> String {
        let data_type = self.inner.data_type().to_string();
        let not_null = if !self.inner.is_nullable() {
            " (not null)"
        } else {
            ""
        };
        format!("{}: {}{}", self.inner.name(), data_type, not_null)
    }

    fn __str__(&self) -> String {
        let data_type = self.inner.data_type().to_string();
        let not_null = if !self.inner.is_nullable() {
            " (not null)"
        } else {
            ""
        };
        format!("{}: {}{}", self.inner.name(), data_type, not_null)
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PySchema {
    inner: SchemaRef,
}

#[pymethods]
impl PySchema {
    fn is_empty(&self) -> bool {
        self.inner.fields().is_empty()
    }

    #[getter]
    fn fields(&self) -> Vec<PySchemaField> {
        self.inner
            .fields
            .iter()
            .map(|f| PySchemaField { inner: f.clone() })
            .collect()
    }

    fn field(&self, name: &str) -> PyResult<PySchemaField> {
        self.inner
            .field_with_name(name)
            .map(|f| PySchemaField {
                inner: FieldRef::new(f.clone()),
            })
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    }

    fn __repr__(&self) -> String {
        self._format_schema()
    }

    fn __str__(&self) -> String {
        self._format_schema()
    }

    fn _format_schema(&self) -> String {
        if self.inner.fields().is_empty() {
            return String::new();
        }

        let formatted_fields: Vec<String> = self
            .inner
            .fields()
            .iter()
            .map(|field| {
                let data_type = field.data_type().to_string();
                let not_null = if !field.is_nullable() {
                    " (not null)"
                } else {
                    ""
                };
                format!("- {}: {}{}", field.name(), data_type, not_null)
            })
            .collect();

        formatted_fields.join("\n")
    }

    fn __getitem__(&self, idx: usize) -> PyResult<PySchemaField> {
        self.inner
            .fields()
            .iter()
            .nth(idx)
            .map(|field| PySchemaField {
                inner: field.clone(),
            })
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyIndexError, _>(format!(
                    "Schema index out of range: {}",
                    idx
                ))
            })
    }

    fn __len__(&self) -> usize {
        self.inner.fields().len()
    }
}

impl PySchema {
    pub fn new(schema: SchemaRef) -> Self {
        PySchema { inner: schema }
    }
}
