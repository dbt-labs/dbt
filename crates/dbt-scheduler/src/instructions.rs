use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow_schema::SchemaRef;
use dbt_adapter_core::AdapterType;
use dbt_common::CompiledSpans;

#[derive(Debug, Default, Clone)]
pub struct Instructions {
    pub sql_instructions: BTreeMap<String, SqlInstruction>,
    pub lp_instructions: BTreeMap<String, LpInstruction>,
}

impl fmt::Display for Instructions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_sql(f, &self.sql_instructions)?;
        format_lp(f, &self.lp_instructions)
    }
}

impl Instructions {
    pub fn iter(&self) -> impl Iterator<Item = (&str, Instruction)> {
        self.sql_instructions
            .iter()
            .map(|(k, v)| (k.as_str(), Instruction::Sql(v.clone())))
            .collect::<Vec<_>>()
            .into_iter()
            .chain(
                self.lp_instructions
                    .iter()
                    .map(|(k, v)| (k.as_str(), Instruction::Lp(v.clone())))
                    .collect::<Vec<_>>(),
            )
    }

    pub fn get_lp_or_sql(&self, unique_id: &str) -> Option<Instruction> {
        self.lp_instructions
            .get(unique_id)
            .map(|i| Instruction::Lp(i.clone()))
            .or_else(|| {
                self.sql_instructions
                    .get(unique_id)
                    .map(|i| Instruction::Sql(i.clone()))
            })
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Instruction {
    Sql(SqlInstruction),
    Lp(LpInstruction),
}

// We use SqlInstructions for Tier1 and 2
#[derive(Debug, Clone, Default)]
pub struct SqlInstruction {
    pub fqn: Vec<String>,
    pub sql: String,
    pub original_path: PathBuf,
    pub spans: Box<dyn CompiledSpans>,
}

// We use SqlInstructions for Tier3 and 4
#[derive(Debug, Clone)]
pub struct LpInstruction {
    pub fqn: Vec<String>,
    pub plan: LpPlan,
    pub original_path: PathBuf,
}

/// The bound plan of an `Lp` instruction. The out-of-process frontend that
/// bound it keeps the plan; this carries what executors read from it.
#[derive(Debug, Clone)]
pub struct LpPlan(pub Arc<RemotePlan>);

/// What executors read from a plan held by an out-of-process frontend.
#[derive(Debug)]
pub struct RemotePlan {
    /// The plan's output schema; `None` for statements that return no rows.
    pub output_schema: Option<SchemaRef>,
    /// The plan as text, for logs.
    pub display: String,
    /// The plan emitted as SQL for the local engine, by the source adapter
    /// whose dialect it was emitted from. `Err` holds the emitter's message.
    pub emitted_sql: HashMap<AdapterType, Result<String, String>>,
}

impl LpPlan {
    /// The plan's output schema; `None` for statements that return no rows
    /// (DDL, DML, COPY, EXPLAIN, ...).
    pub fn output_schema(&self) -> Option<SchemaRef> {
        self.0.output_schema.clone()
    }

    /// The plan as text, for logs.
    pub fn display(&self) -> String {
        self.0.display.clone()
    }
}

impl Instruction {
    pub fn fqn(&self) -> &Vec<String> {
        match self {
            Instruction::Sql(sql_instruction) => &sql_instruction.fqn,
            Instruction::Lp(lp_instruction) => &lp_instruction.fqn,
        }
    }

    pub fn original_path(&self) -> &Path {
        match self {
            Instruction::Sql(sql_instruction) => sql_instruction.original_path.as_path(),
            Instruction::Lp(lp_instruction) => lp_instruction.original_path.as_path(),
        }
    }
}

fn format_sql(
    f: &mut fmt::Formatter<'_>,
    sql_instructions: &BTreeMap<String, SqlInstruction>,
) -> fmt::Result {
    // Ensure we print in the same order across multiple invocations.
    let mut keys: Vec<String> = sql_instructions.keys().cloned().collect();
    keys.sort();

    for unique_id in keys {
        let sql_instruction = sql_instructions.get(&unique_id).unwrap();
        writeln!(
            f,
            "--  ID: {}, FQN: {:?}, PATH: {} (SQL)",
            unique_id,
            sql_instruction.fqn.join("."),
            sql_instruction.original_path.display()
        )?;
        let cleaned_sql: String = sql_instruction
            .sql
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<&str>>()
            .join("\n    ");
        writeln!(f, "    {cleaned_sql}")?;
    }
    Ok(())
}

fn format_lp(
    f: &mut fmt::Formatter<'_>,
    lp_instructions: &BTreeMap<String, LpInstruction>,
) -> fmt::Result {
    for (unique_id, lp_instruction) in lp_instructions {
        writeln!(
            f,
            "--  ID: {}, FQN: {:?}, PATH: {} (Logical Plan)",
            unique_id,
            lp_instruction.fqn.join("."),
            lp_instruction.original_path.display()
        )?;
    }
    Ok(())
}
