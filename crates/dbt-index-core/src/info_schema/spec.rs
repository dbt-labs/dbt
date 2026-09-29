//! Declarative description of the information schema.
//!
//! Each output table is described once, as a list of output columns paired with
//! the source column they are taken from. The output Arrow type is *derived*
//! from the source field rather than restated here, so a rename cannot change a
//! column's type by accident and there is no second copy of ~450 type
//! declarations to keep in step.
//!
//! Columns whose data has no source yet are declared with an explicit type and
//! emitted null-filled, so the shape a consumer sees is stable even before the
//! data exists.

use arrow_schema::DataType;

/// SQL namespace an output table belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ns {
    /// Project metadata. Snapshot semantics.
    Dbt,
    /// Runtime results. Replaced per invocation.
    DbtRt,
    /// Not part of the public contract; shape may change without notice.
    DbtInternal,
}

impl Ns {
    pub const fn prefix(self) -> &'static str {
        match self {
            Ns::Dbt => "dbt",
            Ns::DbtRt => "dbt_rt",
            Ns::DbtInternal => "dbt_internal",
        }
    }

    pub const ALL: &'static [Ns] = &[Ns::Dbt, Ns::DbtRt, Ns::DbtInternal];
}

/// Where an output table's rows come from.
#[derive(Debug, Clone, Copy)]
pub enum Src {
    /// Project a single source table.
    Table(&'static str),
    /// Left-join two source tables. Output columns resolve against `left`
    /// first, then `right`.
    Join {
        left: &'static str,
        right: &'static str,
        left_on: &'static str,
        right_on: &'static str,
    },
    /// Rows are assembled by dedicated code rather than by column projection.
    Own,
}

/// Row filter applied before projection.
#[derive(Debug, Clone, Copy)]
pub enum Filter {
    All,
    /// Keep rows whose `resource_type` is in the list.
    ResourceTypeIn(&'static [&'static str]),
}

/// Type of a column that has no source column to derive from.
#[derive(Debug, Clone, Copy)]
pub enum ColTy {
    Utf8,
    Bool,
    I64,
    ListUtf8,
    TsUtc,
}

impl ColTy {
    pub fn data_type(self) -> DataType {
        use arrow_schema::{Field, TimeUnit};
        use std::sync::Arc;
        match self {
            ColTy::Utf8 => DataType::Utf8,
            ColTy::Bool => DataType::Boolean,
            ColTy::I64 => DataType::Int64,
            ColTy::ListUtf8 => DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            ColTy::TsUtc => DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
        }
    }
}

/// One output column.
pub struct ColSpec {
    /// Name in the information schema.
    pub out: &'static str,
    /// Name in the source table. Empty when `ty` is set.
    pub src: &'static str,
    /// Set only for columns with no source; such columns are null-filled.
    pub ty: Option<ColTy>,
}

/// One output table.
pub struct TableSpec {
    pub ns: Ns,
    pub name: &'static str,
    pub src: Src,
    pub filter: Filter,
    pub cols: &'static [ColSpec],
}

impl TableSpec {
    /// `dbt.models` -> `dbt.models.parquet`
    pub fn file_name(&self) -> String {
        format!("{}.{}.parquet", self.ns.prefix(), self.name)
    }

    /// `dbt.models`
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.ns.prefix(), self.name)
    }
}

/// Carried through unchanged.
pub const fn c(out: &'static str) -> ColSpec {
    ColSpec {
        out,
        src: out,
        ty: None,
    }
}

/// Renamed from `src`.
pub const fn r(out: &'static str, src: &'static str) -> ColSpec {
    ColSpec { out, src, ty: None }
}

/// Declared with a type but no source; emitted null-filled.
pub const fn n(out: &'static str, ty: ColTy) -> ColSpec {
    ColSpec {
        out,
        src: "",
        ty: Some(ty),
    }
}
