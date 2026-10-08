//! Interactive prompts (inquire) with Traditional Chinese help texts.

use anyhow::{Result, anyhow};
use inquire::validator::Validation;
use inquire::{Confirm, InquireError, Password, PasswordDisplayMode, Select, Text};
use sshelper_core::deploy::TargetOs;
use sshelper_core::keys::KeyPair;

/// The user aborted a prompt (Esc / Ctrl+C) or declined to continue.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("已取消")
    }
}

impl std::error::Error for Cancelled {}

fn answer<T>(result: Result<T, InquireError>) -> Result<T> {
    result.map_err(|e| match e {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => anyhow!(Cancelled),
        other => anyhow!("無法讀取輸入：{other}"),
    })
}

pub fn select_key(pairs: &[KeyPair]) -> Result<KeyPair> {
    let usable: Vec<&KeyPair> = pairs.iter().filter(|k| k.problem().is_none()).collect();
    let options: Vec<String> = usable.iter().map(|k| k.describe()).collect();
    let choice = answer(
        Select::new("要部署哪一把公鑰？", options)
            .with_help_message("↑↓ 選擇、Enter 確認、輸入文字可篩選")
            .raw_prompt(),
    )?;
    Ok(usable[choice.index].clone())
}

/// Free text with a validator from `sshelper_core::validate`.
pub fn text<V>(message: &str, default: Option<&str>, help: Option<&str>, validator: V) -> Result<String>
where
    V: Fn(&str) -> Result<(), String> + Clone + 'static,
{
    let mut prompt = Text::new(message).with_validator(move |input: &str| {
        Ok(match validator(input.trim()) {
            Ok(()) => Validation::Valid,
            Err(message) => Validation::Invalid(message.into()),
        })
    });
    if let Some(default) = default {
        prompt = prompt.with_default(default);
    }
    if let Some(help) = help {
        prompt = prompt.with_help_message(help);
    }
    let value: String = answer(prompt.prompt())?;
    Ok(value.trim().to_owned())
}

pub fn target_os() -> Result<TargetOs> {
    let options: Vec<&str> = TargetOs::ALL
        .iter()
        .map(|os| match os {
            TargetOs::Auto => "自動偵測（建議）",
            other => other.label(),
        })
        .collect();
    let choice = answer(
        Select::new("目標主機作業系統？", options)
            .with_help_message("自動偵測會依遠端家目錄判斷 Linux / Windows")
            .raw_prompt(),
    )?;
    Ok(TargetOs::ALL[choice.index])
}

pub fn confirm(message: &str, default: bool) -> Result<bool> {
    answer(Confirm::new(message).with_default(default).prompt())
}

pub fn password(message: &str) -> Result<String> {
    answer(
        Password::new(message)
            .without_confirmation()
            .with_display_mode(PasswordDisplayMode::Masked)
            .prompt(),
    )
}
