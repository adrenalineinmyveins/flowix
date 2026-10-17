//! F6 L1 规则引擎演示：对真实文本执行脱敏并打印报告。
//!
//! 用法：
//!   cargo run -p flowix-redaction --example demo
//!   cargo run -p flowix-redaction --example demo -- --file 笔记.md
//!   cargo run -p flowix-redaction --example demo -- --file 笔记.md --words 词表.txt
//!
//! 无参数时跑内置对抗样本（覆盖全部 8 类规则 + L3 复扫拦截演示）。

use std::collections::BTreeMap;

use chrono::NaiveDate;
use flowix_redaction::{
    check_placeholder_integrity, find_placeholders, redact, scan_with_map, CustomWord,
    RedactionOptions,
};

fn demo_words() -> Vec<CustomWord> {
    [
        "王五=>客户甲",
        "内部代号X7",
    ]
    .iter()
    .map(|line| flowix_redaction::parse_word_line(line).unwrap())
    .collect()
}

fn demo_text() -> String {
    let mut text = String::from("---\nauthor: 王五\nphone: 13912345678\n---\n");
    text.push_str("王五上周找我核对报销：手机 13800138000，邮箱 bob.smith@example.co.uk。\n");
    text.push_str("身份证 11010519491231002X，工资卡 4111111111111111，备用卡 6222020200112233。\n");
    text.push_str("发票存在 /Users/wangwu/Documents/发票2025.pdf，本机路径 C:\\Users\\wang\\Desktop\\税单.xlsx，");
    text.push_str("云端 ~/notes/reimburse.md。\n");
    text.push_str("回调接口 https://api.example.com/v1/cb?token=sk-9f2b&page=2&sign=s1a2。\n");
    text.push_str("时间线：2025-03-14 初审，2025年3月19日 复核，2025/03/21 09:30 打款。\n");
    text.push_str("伪造占位符 ⟦PHONE-7⟧ 混入测试。\n");
    text
}

fn print_report(title: &str, text: &str, words: &[CustomWord]) -> flowix_redaction::RedactionReport {
    let options = RedactionOptions {
        strict_luhn: false,
        rewrite_absolute_time: true,
        now: Some(NaiveDate::from_ymd_opt(2025, 3, 21).unwrap()),
    };
    let report = redact(text, &options, words);

    println!("════════ {title} ════════");
    println!("\n── 脱敏输出 ──\n{}", report.text);

    println!("\n── 规则命中 ──");
    if report.hits.is_empty() {
        println!("（无命中）");
    }
    for hit in &report.hits {
        println!("  {:<12} × {}", hit.rule_id, hit.count);
    }

    println!("\n── 占位符映射表（仅本地留档，不出设备）──");
    if report.placeholder_map.is_empty() {
        println!("（无）");
    }
    for (token, original) in &report.placeholder_map {
        println!("  {token} ⇐ {original}");
    }
    println!();
    report
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file: Option<String> = None;
    let mut words_file: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--file" => file = iter.next().cloned(),
            "--words" => words_file = iter.next().cloned(),
            other => {
                eprintln!("未知参数: {other}（支持 --file <路径> 与 --words <词表>）");
                std::process::exit(2);
            }
        }
    }

    let words: Vec<CustomWord> = match &words_file {
        Some(path) => std::fs::read_to_string(path)
            .unwrap_or_else(|e| {
                eprintln!("词表读取失败 {path}: {e}");
                std::process::exit(2);
            })
            .lines()
            .filter_map(flowix_redaction::parse_word_line)
            .collect(),
        None => {
            if file.is_some() {
                Vec::new()
            } else {
                demo_words()
            }
        }
    };

    let options = RedactionOptions {
        strict_luhn: false,
        rewrite_absolute_time: true,
        now: Some(NaiveDate::from_ymd_opt(2025, 3, 21).unwrap()),
    };

    match file {
        Some(path) => {
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("文件读取失败 {path}: {e}");
                std::process::exit(2);
            });
            print_report(&format!("L1 脱敏 · {path}"), &text, &words);
        }
        None => {
            let text = demo_text();
            let report = print_report("L1 脱敏 · 内置对抗样本", &text, &words);

            println!("════════ L3 复扫演示（模拟 LLM 蒸馏后把敏感项写回来）════════");
            let mut tampered = report.text.replace("⟦PHONE-1⟧", "13800138000");
            tampered.push_str("\n蒸馏器补充：联系方式 ⟦EMAIL-99⟧");
            let hits = scan_with_map(&tampered, &options, &words, &report.placeholder_map);
            println!("\n── 复扫命中（应拦截回炉）──");
            for hit in &hits {
                println!("  {:<12} × {}", hit.rule_id, hit.count);
            }
            let mut map: BTreeMap<String, String> = report.placeholder_map.clone();
            map.remove("⟦EMAIL-1⟧");
            let problems = check_placeholder_integrity(&tampered, &map);
            println!("\n── 占位符对账 ──");
            for problem in &problems {
                println!("  {problem:?}");
            }
            println!("\n── 蒸馏稿中的占位符清单 ──");
            for token in find_placeholders(&report.text) {
                println!("  {token}");
            }
        }
    }
}
