use std::collections::BTreeSet;

use anyhow::Result;
use clap::Args;
use tracing::instrument;

use crate::doctor::runner::{GroupOrderParams, compute_group_order};
use crate::report_stdout;
use crate::shared::prelude::{DoctorGroup, FoundConfig, print_details_with_column};

#[derive(Debug, Args)]
pub struct DoctorListArgs {}

#[instrument("scope doctor list", skip_all)]
pub async fn doctor_list(found_config: &FoundConfig, _args: &DoctorListArgs) -> Result<()> {
    report_stdout!("Available checks that will run");
    let order = generate_doctor_list(found_config).clone();
    let included = |group: &DoctorGroup| {
        if group.run_by_default {
            "Yes".to_string()
        } else {
            "No".to_string()
        }
    };
    print_details_with_column(
        &found_config.working_dir,
        &order,
        Some(("Included", &included)),
    )
    .await;
    Ok(())
}

pub fn generate_doctor_list(found_config: &FoundConfig) -> Vec<DoctorGroup> {
    let all_keys = BTreeSet::from_iter(found_config.doctor_group.keys().map(|k| k.to_string()));
    let group_order = compute_group_order(GroupOrderParams {
        groups: &found_config.doctor_group,
        desired_groups: &all_keys,
        skip_subtree: &BTreeSet::new(),
        skip_only: &BTreeSet::new(),
    });

    group_order
        .iter()
        .map(|name| found_config.doctor_group.get(name).unwrap().clone())
        .collect()
}
