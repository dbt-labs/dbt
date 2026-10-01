use std::collections::HashMap;
use std::fmt;

use crate::schemas::BatchResults;
use crate::schemas::Nodes;
use dbt_common::stats::Stat;
use humantime::format_duration;

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub stats: Vec<Stat>,
    pub nodes: Option<Nodes>,
    pub batch_results: HashMap<String, BatchResults>,
    pub compiled_code: HashMap<String, String>,
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let all_num_rows_none = self.stats.iter().all(|stat| stat.num_rows.is_none());

        // Calculate the maximum length of the unique_id values
        let max_unique_id_len = self
            .stats
            .iter()
            .map(|stat| stat.unique_id.len())
            .max()
            .unwrap_or(0); // Default to 35 if no stats are present

        if all_num_rows_none {
            writeln!(
                f,
                "{:<width$} | {:<9} | {:<10} | {:<10} | {:<10}",
                "Unique ID",
                "Status",
                "Start Time",
                "End Time",
                "Duration",
                width = max_unique_id_len
            )?;
            // Calculate the total width of the line
            let total_width = max_unique_id_len + 3 + 9 + 3 + 10 + 3 + 10 + 3 + 10; // 3 is for the spaces and separators

            writeln!(f, "{}", "-".repeat(total_width))?;
            for stat in &self.stats {
                writeln!(
                    f,
                    "{:<width$} | {:<9} | {:<10} | {:<10} | {:<10}",
                    stat.unique_id,
                    stat.status_string(),
                    Stat::format_time(stat.start_time),
                    Stat::format_time(stat.end_time),
                    format_duration(stat.get_duration()).to_string(),
                    width = max_unique_id_len
                )?;
            }
        } else {
            writeln!(
                f,
                "{:<width$} | {:<9} | {:<8} | {:<10} | {:<10} | {:<10}",
                "Unique ID",
                "Status",
                "Num Rows",
                "Start Time",
                "End Time",
                "Duration",
                width = max_unique_id_len
            )?;
            let total_width = max_unique_id_len + 3 + 9 + 3 + 8 + 3 + 10 + 3 + 10; // 3 is for the spaces and separators

            writeln!(f, "{}", "-".repeat(total_width))?;

            for stat in &self.stats {
                writeln!(
                    f,
                    "{:<width$} | {:<9} | {:<8} | {:<10} | {:<10} | {:<10}",
                    stat.unique_id,
                    stat.status_string(),
                    stat.num_rows
                        .map_or_else(|| "-".to_string(), |num| num.to_string()),
                    Stat::format_time(stat.start_time),
                    Stat::format_time(stat.end_time),
                    format_duration(stat.get_duration()).to_string(),
                    width = max_unique_id_len
                )?;
            }
        }
        Ok(())
    }
}
