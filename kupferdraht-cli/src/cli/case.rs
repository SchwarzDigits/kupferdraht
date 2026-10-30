// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `case-set` / `case-show` — per-room case state (Step 72c,
//! ADR-0018). The set command patches the folded state and sends a
//! full snapshot; show prints the current fold.

use kupferdraht::KupferdrahtError;
use kupferdraht::timeline::{CasePatch, CasePriority, CaseSnapshot, CaseState};

use super::resume_client;

fn parse_state(raw: &str) -> Result<CaseState, KupferdrahtError> {
    Ok(match raw {
        "open" => CaseState::Open,
        "active" => CaseState::Active,
        "waiting" => CaseState::Waiting,
        "resolved" => CaseState::Resolved,
        "closed" => CaseState::Closed,
        "cancelled" => CaseState::Cancelled,
        other => {
            return Err(KupferdrahtError::InvalidInput(format!(
                "case-set: unknown state `{other}` (open|active|waiting|resolved|closed|cancelled)"
            )));
        }
    })
}

fn parse_priority(raw: &str) -> Result<CasePriority, KupferdrahtError> {
    Ok(match raw {
        "low" => CasePriority::Low,
        "normal" => CasePriority::Normal,
        "high" => CasePriority::High,
        "urgent" => CasePriority::Urgent,
        other => {
            return Err(KupferdrahtError::InvalidInput(format!(
                "case-set: unknown priority `{other}` (low|normal|high|urgent)"
            )));
        }
    })
}

fn parse_extensions(
    pairs: &[String],
) -> Result<std::collections::HashMap<String, String>, KupferdrahtError> {
    let mut out = std::collections::HashMap::new();
    for pair in pairs {
        let Some((k, v)) = pair.split_once('=') else {
            return Err(KupferdrahtError::InvalidInput(format!(
                "case-set: --ext expects KEY=VALUE, got `{pair}`"
            )));
        };
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

fn print_case(prefix: &str, case: &CaseSnapshot) {
    println!("{prefix}");
    println!("  case_ref:    {}", case.case_ref);
    println!("  revision:    {}", case.revision);
    if let Some(v) = &case.external_ref {
        println!("  external:    {v}");
    }
    if let Some(v) = &case.title {
        println!("  title:       {v}");
    }
    if let Some(v) = &case.case_type {
        println!("  type:        {v}");
    }
    println!("  state:       {:?}", case.state);
    if let Some(v) = &case.state_detail {
        println!("  detail:      {v}");
    }
    if let Some(v) = &case.assignee {
        println!("  assignee:    {v}");
    }
    if let Some(v) = case.due_at_ms {
        println!("  due_at_ms:   {v}");
    }
    if let Some(v) = case.priority {
        println!("  priority:    {v:?}");
    }
    if let Some(v) = &case.authority_ref {
        println!("  authority:   {v}");
    }
    if !case.extensions.is_empty() {
        println!("  extensions:");
        let mut keys: Vec<_> = case.extensions.keys().collect();
        keys.sort();
        for k in keys {
            println!("    {k} = {}", case.extensions[k]);
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::struct_excessive_bools)]
pub(crate) struct CaseSetArgs {
    pub room_id: String,
    pub case_ref: Option<String>,
    pub external_ref: Option<String>,
    pub title: Option<String>,
    pub case_type: Option<String>,
    pub state: Option<String>,
    pub detail: Option<String>,
    pub assignee: Option<String>,
    pub due_ms: Option<i64>,
    pub priority: Option<String>,
    pub authority: Option<String>,
    pub ext: Vec<String>,
}

pub(crate) async fn case_set(args: CaseSetArgs) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_metadata().await?;
    let room = client.room(&args.room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("case-set: room {} not found", args.room_id))
    })?;
    let patch = CasePatch {
        case_ref: args.case_ref,
        external_ref: args.external_ref,
        title: args.title,
        case_type: args.case_type,
        state: args.state.as_deref().map(parse_state).transpose()?,
        state_detail: args.detail,
        assignee: args.assignee,
        due_at_ms: args.due_ms,
        priority: args.priority.as_deref().map(parse_priority).transpose()?,
        opened_at_ms: None,
        closed_at_ms: None,
        authority_ref: args.authority,
        extensions: if args.ext.is_empty() {
            None
        } else {
            Some(parse_extensions(&args.ext)?)
        },
    };
    let snapshot = room.set_case(patch).await?;
    print_case("case-set ok", &snapshot);
    Ok(())
}

pub(crate) async fn case_show(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await?;
    let room = client.room(&room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("case-show: room {room_id} not found"))
    })?;
    match room.case().await? {
        Some(case) => print_case("case-show", &case),
        None => println!("case-show: no case in {room_id}"),
    }
    Ok(())
}
