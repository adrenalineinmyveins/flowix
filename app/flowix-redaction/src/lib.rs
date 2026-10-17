//! F6 L1 规则脱敏引擎（REDACTION-PIPELINE.md §3.2）。
//!
//! 纯函数 crate：零 Tauri / 零 flowix-core 依赖，输入输出均为内存字符串。
//! 规则目录内置版本化（`RULE_CATALOG_VERSION`），词表由调用方加载后传入。
//!
//! 处理顺序（固定，影响占位符编号）：
//! frontmatter 剥离 → 占位符中和 → 自定义词表 → 邮箱 → URL 凭据 →
//! 文件路径 → 身份证/卡号/手机（极大数字串一次分类）→ 绝对时间相对化。
//!
//! 占位符语义：`⟦KIND-n⟧` 按类别独立编号，同类别相同原文复用同一编号；
//! 映射表只随 `RedactionReport` 返回（本地留档用），回复文本默认不回填。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// 规则目录版本，随对抗样本集共同演进；写入 outbox 留档用于解释旧记录。
pub const RULE_CATALOG_VERSION: &str = "l1-v1";

const RULE_ORDER: &[&str] = &[
    "frontmatter", "placeholder", "word", "email", "urlsecret", "path", "id", "card", "phone",
    "time",
];

const KIND_PLACEHOLDER: &str = "PLACEHOLDER";
const KIND_WORD: &str = "WORD";
const KIND_EMAIL: &str = "EMAIL";
const KIND_URLSECRET: &str = "URLSECRET";
const KIND_PATH: &str = "PATH";
const KIND_ID: &str = "ID";
const KIND_CARD: &str = "CARD";
const KIND_PHONE: &str = "PHONE";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionOptions {
    /// 银行卡候选须过 Luhn 校验才脱敏（收紧模式，默认关 = 召回优先）。
    pub strict_luhn: bool,
    /// 绝对时间原位改写为相对表述；需要 `now` 作为参照日。
    pub rewrite_absolute_time: bool,
    /// 时间规则参照日；None 时时间规则整体停用。
    pub now: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CustomWord {
    pub original: String,
    pub replacement: Option<String>,
}

/// 解析 words.txt 的一行：`原文=>替换` 指定替换，裸行自动占位符；空行返回 None。
pub fn parse_word_line(line: &str) -> Option<CustomWord> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.split_once("=>") {
        Some((original, replacement)) => {
            let original = original.trim();
            if original.is_empty() {
                return None;
            }
            let replacement = replacement.trim();
            Some(CustomWord {
                original: original.to_string(),
                replacement: (!replacement.is_empty()).then(|| replacement.to_string()),
            })
        }
        None => Some(CustomWord {
            original: trimmed.to_string(),
            replacement: None,
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleHit {
    pub rule_id: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionReport {
    pub text: String,
    pub placeholder_map: BTreeMap<String, String>,
    pub hits: Vec<RuleHit>,
    pub rule_set_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceholderProblem {
    /// 文本中出现但不在映射表的占位符（LLM 拼凑或外部注入）。
    UnknownInText(String),
    /// 映射表中有、文本中缺失的占位符（LLM 丢弃）。
    MissingFromText(String),
}

fn re_poison() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"⟦[A-Z][A-Z0-9]*-[0-9]+⟧").unwrap())
}

fn re_email() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap()
    })
}

fn re_url_secret() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)([?&][A-Za-z0-9_.\-]*(?:token|key|sign)[A-Za-z0-9_.\-]*=)([A-Za-z0-9_.\-~%!$*'()+,;=:@/\[\]]+)",
        )
        .unwrap()
    })
}

fn re_path() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:/Users/|C:[/\\]Users[/\\]|~/)[^\s\u{3000}，。；：！？、“”‘’（）【】《》「」『』⟦⟧]+",
        )
        .unwrap()
    })
}

fn re_digit_run() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[0-9]+").unwrap())
}

fn re_time() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"([0-9]{4})([-/年])([0-9]{1,2})([-/月])([0-9]{1,2})日?(?:[ T][0-9]{1,2}:[0-9]{2}(?::[0-9]{2})?)?",
        )
        .unwrap()
    })
}

fn re_frontmatter() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\u{FEFF}?(?:[ \t]*\r?\n)*---\r?\n([\s\S]*?)(?:\r?\n)?---\r?\n?([\s\S]*)$")
            .unwrap()
    })
}

/// 切掉文档头 YAML frontmatter 块，边界与 flowix-core `FRONTMATTER_RE` 等价。
/// 返回 (frontmatter 块（含定界符，无则 None), body)。
pub fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    match re_frontmatter().captures(content) {
        Some(caps) => {
            let body_len = caps.get(2).map(|m| m.as_str().len()).unwrap_or(0);
            let body_start = content.len() - body_len;
            (Some(&content[..body_start]), &content[body_start..])
        }
        None => (None, content),
    }
}

/// Luhn（模 10 加权）校验，银行卡收紧模式用。
pub fn luhn_valid(digits: &str) -> bool {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for b in digits.bytes().rev() {
        let mut d = (b - b'0') as u32;
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum % 10 == 0
}

fn id18_birth_valid(eighteen: &str) -> bool {
    let bytes = eighteen.as_bytes();
    if bytes.len() < 14 || !bytes[..14].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let year = parse_u32(&bytes[6..10]);
    let month = parse_u32(&bytes[10..12]);
    let day = parse_u32(&bytes[12..14]);
    date_valid(year, month, day)
}

fn id15_birth_valid(fifteen: &str) -> bool {
    let bytes = fifteen.as_bytes();
    if bytes.len() < 12 || !bytes[..12].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let yy = parse_u32(&bytes[6..8]);
    let month = parse_u32(&bytes[8..10]);
    let day = parse_u32(&bytes[10..12]);
    date_valid(1900 + yy, month, day)
}

fn parse_u32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0u32, |acc, b| acc * 10 + u32::from(b - b'0'))
}

fn date_valid(year: u32, month: u32, day: u32) -> bool {
    (1900..=2099).contains(&year) && NaiveDate::from_ymd_opt(year as i32, month, day).is_some()
}

fn cn_phone_shape(eleven: &str) -> bool {
    let b = eleven.as_bytes();
    b.len() == 11 && b[0] == b'1' && (b'3'..=b'9').contains(&b[1])
}

fn weekday_cn(date: NaiveDate) -> &'static str {
    use chrono::Datelike;
    use chrono::Weekday::*;
    match date.weekday() {
        Mon => "一",
        Tue => "二",
        Wed => "三",
        Thu => "四",
        Fri => "五",
        Sat => "六",
        Sun => "日",
    }
}

fn relative_date_text(date: NaiveDate, now: NaiveDate) -> String {
    let ago = (now - date).num_days();
    if ago == 0 {
        "今天".to_string()
    } else if ago == 1 {
        "昨天".to_string()
    } else if ago == 2 {
        "前天".to_string()
    } else if ago == -1 {
        "明天".to_string()
    } else if ago == -2 {
        "后天".to_string()
    } else if (3..=6).contains(&ago) {
        format!("{ago}天前")
    } else if (-6..=-3).contains(&ago) {
        format!("{}天后", -ago)
    } else if (7..=13).contains(&ago) {
        format!("上周{}", weekday_cn(date))
    } else if (-13..=-7).contains(&ago) {
        format!("下周{}", weekday_cn(date))
    } else if (14..=29).contains(&ago) {
        format!("{}周前", ago / 7)
    } else if (-29..=-14).contains(&ago) {
        format!("{}周后", -ago / 7)
    } else if (30..=364).contains(&ago) {
        format!("{}个月前", ago / 30)
    } else if (-364..=-30).contains(&ago) {
        format!("{}个月后", -ago / 30)
    } else if ago > 0 {
        format!("{}年前", ago / 365)
    } else {
        format!("{}年后", -ago / 365)
    }
}

fn bracket_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ch) in text.char_indices() {
        match ch {
            '⟦' => {
                if start.is_none() {
                    start = Some(i);
                }
            }
            '⟧' => {
                if let Some(s) = start.take() {
                    spans.push((s, i + '⟧'.len_utf8()));
                }
            }
            _ => {}
        }
    }
    spans
}

fn overlaps_guard(start: usize, end: usize, guard: &[(usize, usize)]) -> bool {
    guard.iter().any(|(s, e)| start < *e && *s < end)
}

fn splice(text: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|(s, _, _)| *s);
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for (s, e, rep) in edits {
        if s < last {
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(&rep);
        last = e;
    }
    out.push_str(&text[last..]);
    out
}

struct Redactor<'a> {
    options: &'a RedactionOptions,
    words: Vec<&'a CustomWord>,
    allowed_placeholders: &'a BTreeMap<String, String>,
    map: BTreeMap<String, String>,
    reverse: BTreeMap<(&'static str, String), String>,
    counters: BTreeMap<&'static str, usize>,
    hits: BTreeMap<&'static str, usize>,
}

impl<'a> Redactor<'a> {
    fn new(
        options: &'a RedactionOptions,
        custom_words: &'a [CustomWord],
        allowed_placeholders: &'a BTreeMap<String, String>,
    ) -> Self {
        let mut words: Vec<&CustomWord> = custom_words.iter().collect();
        words.sort_by(|a, b| {
            b.original
                .len()
                .cmp(&a.original.len())
                .then_with(|| a.original.cmp(&b.original))
        });
        Self {
            options,
            words,
            allowed_placeholders,
            map: BTreeMap::new(),
            reverse: BTreeMap::new(),
            counters: BTreeMap::new(),
            hits: BTreeMap::new(),
        }
    }

    fn hit(&mut self, rule: &'static str) {
        *self.hits.entry(rule).or_insert(0) += 1;
    }

    fn placeholder(&mut self, kind: &'static str, original: &str) -> String {
        let key = (kind, original.to_string());
        if let Some(existing) = self.reverse.get(&key) {
            return existing.clone();
        }
        let n = self.counters.entry(kind).or_insert(0);
        *n += 1;
        let token = format!("⟦{kind}-{n}⟧");
        self.map.insert(token.clone(), original.to_string());
        self.reverse.insert(key, token.clone());
        token
    }

    fn neutralize_placeholders(&mut self, text: &str) -> String {
        let mut edits = Vec::new();
        for m in re_poison().find_iter(text) {
            if self.allowed_placeholders.contains_key(m.as_str()) {
                continue;
            }
            let token = self.placeholder(KIND_PLACEHOLDER, m.as_str());
            edits.push((m.start(), m.end(), token));
            self.hit("placeholder");
        }
        splice(text, edits)
    }

    fn apply_word_rules(&mut self, text: &str) -> String {
        let mut current = text.to_string();
        let sorted_words: Vec<&'a CustomWord> = self.words.clone();
        for word in sorted_words {
            let guard = bracket_spans(&current);
            let mut edits = Vec::new();
            let mut from = 0usize;
            while let Some(rel) = current[from..].find(&word.original) {
                let start = from + rel;
                let end = start + word.original.len();
                from = end;
                if overlaps_guard(start, end, &guard) {
                    continue;
                }
                let replacement = match &word.replacement {
                    Some(fixed) => fixed.clone(),
                    None => self.placeholder(KIND_WORD, &word.original),
                };
                edits.push((start, end, replacement));
                self.hit("word");
            }
            if !edits.is_empty() {
                current = splice(&current, edits);
            }
        }
        current
    }

    fn regex_pass(
        &mut self,
        text: &str,
        re: &Regex,
        rule: &'static str,
        kind: &'static str,
        value_group: usize,
        prefix_group: Option<usize>,
    ) -> String {
        let guard = bracket_spans(text);
        let mut edits = Vec::new();
        for caps in re.captures_iter(text) {
            let whole = match caps.get(0) {
                Some(w) => w,
                None => continue,
            };
            let value = match caps.get(value_group) {
                Some(v) => v,
                None => continue,
            };
            if overlaps_guard(whole.start(), whole.end(), &guard) {
                continue;
            }
            let token = self.placeholder(kind, value.as_str());
            let start = match prefix_group.and_then(|g| caps.get(g)) {
                Some(p) => p.end(),
                None => whole.start(),
            };
            edits.push((start, whole.end(), token));
            self.hit(rule);
        }
        splice(text, edits)
    }

    fn digit_pass(&mut self, text: &str) -> String {
        let guard = bracket_spans(text);
        let mut edits = Vec::new();
        for m in re_digit_run().find_iter(text) {
            let run = m.as_str();
            let len = run.len();
            let mut end = m.end();
            let mut classified: Option<(&'static str, &'static str)> = None;

            if len == 11 && cn_phone_shape(run) {
                classified = Some(("phone", KIND_PHONE));
            } else if len == 13 && run.starts_with("86") && cn_phone_shape(&run[2..]) {
                classified = Some(("phone", KIND_PHONE));
            } else if len == 15 && id15_birth_valid(run) {
                classified = Some(("id", KIND_ID));
            } else if len == 18 && id18_birth_valid(run) {
                classified = Some(("id", KIND_ID));
            } else if len == 17 {
                let after = text[end..].chars().next();
                if matches!(after, Some('X' | 'x')) {
                    let after_x = text[end + 1..].chars().next();
                    let blocked = matches!(after_x, Some('0'..='9' | 'X' | 'x'));
                    if !blocked {
                        end += 1;
                        classified = Some(("id", KIND_ID));
                    }
                }
            }

            if classified.is_none() && (13..=19).contains(&len) {
                if !self.options.strict_luhn || luhn_valid(run) {
                    classified = Some(("card", KIND_CARD));
                }
            }

            if let Some((rule, kind)) = classified {
                if overlaps_guard(m.start(), end, &guard) {
                    continue;
                }
                let original = &text[m.start()..end];
                let token = self.placeholder(kind, original);
                edits.push((m.start(), end, token));
                self.hit(rule);
            }
        }
        splice(text, edits)
    }

    fn rewrite_time(&mut self, text: &str, now: NaiveDate) -> String {
        let guard = bracket_spans(text);
        let mut edits = Vec::new();
        for caps in re_time().captures_iter(text) {
            let whole = match caps.get(0) {
                Some(w) => w,
                None => continue,
            };
            if overlaps_guard(whole.start(), whole.end(), &guard) {
                continue;
            }
            let prev_digit = text[..whole.start()]
                .bytes()
                .next_back()
                .is_some_and(|b| b.is_ascii_digit());
            let next_digit = text[whole.end()..]
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_digit());
            if prev_digit || next_digit {
                continue;
            }
            let sep_a = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let sep_b = caps.get(4).map(|m| m.as_str()).unwrap_or("");
            let paired =
                (sep_a == sep_b) || (sep_a == "年" && sep_b == "月");
            if !paired {
                continue;
            }
            let year = caps.get(1).and_then(|m| m.as_str().parse::<i32>().ok());
            let month = caps.get(3).and_then(|m| m.as_str().parse::<u32>().ok());
            let day = caps.get(5).and_then(|m| m.as_str().parse::<u32>().ok());
            let date =
                NaiveDate::from_ymd_opt(year.unwrap_or(0), month.unwrap_or(0), day.unwrap_or(0));
            let Some(date) = date else {
                continue;
            };
            let relative = relative_date_text(date, now);
            edits.push((whole.start(), whole.end(), relative));
            self.hit("time");
        }
        splice(text, edits)
    }

    fn run(mut self, input: &str) -> (String, BTreeMap<String, String>, Vec<RuleHit>) {
        let mut text = input.to_string();
        let (frontmatter, body) = split_frontmatter(&text);
        if frontmatter.is_some() {
            self.hit("frontmatter");
            text = body.to_string();
        }
        text = self.neutralize_placeholders(&text);
        text = self.apply_word_rules(&text);
        text = self.regex_pass(&text, re_email(), "email", KIND_EMAIL, 0, None);
        text = self.regex_pass(&text, re_url_secret(), "urlsecret", KIND_URLSECRET, 2, Some(1));
        text = self.regex_pass(&text, re_path(), "path", KIND_PATH, 0, None);
        text = self.digit_pass(&text);
        if self.options.rewrite_absolute_time {
            if let Some(now) = self.options.now {
                text = self.rewrite_time(&text, now);
            }
        }
        let hits = RULE_ORDER
            .iter()
            .filter_map(|rule| {
                self.hits
                    .get(*rule)
                    .map(|count| RuleHit {
                        rule_id: rule.to_string(),
                        count: *count,
                    })
            })
            .collect();
        (text, self.map, hits)
    }
}

/// L1 主入口：frontmatter 剥离 + 全规则改写，返回脱敏文本、占位符映射与命中清单。
pub fn redact(
    input: &str,
    options: &RedactionOptions,
    custom_words: &[CustomWord],
) -> RedactionReport {
    let allowed = BTreeMap::new();
    let (text, placeholder_map, hits) =
        Redactor::new(options, custom_words, &allowed).run(input);
    RedactionReport {
        text,
        placeholder_map,
        hits,
        rule_set_version: RULE_CATALOG_VERSION.to_string(),
    }
}

/// L3 复扫：只统计命中不改写，与 `redact` 走同一代码路径保证口径一致。
/// 文本中的占位符按「外来 token」参与规则（word/email 等命中占位符内部会被区间守卫跳过）。
pub fn scan(text: &str, options: &RedactionOptions, custom_words: &[CustomWord]) -> Vec<RuleHit> {
    scan_with_map(text, options, custom_words, &BTreeMap::new())
}

/// L3 复扫（带合法占位符映射）：映射表内的占位符被豁免，不计入 `placeholder` 规则。
pub fn scan_with_map(
    text: &str,
    options: &RedactionOptions,
    custom_words: &[CustomWord],
    allowed_placeholders: &BTreeMap<String, String>,
) -> Vec<RuleHit> {
    Redactor::new(options, custom_words, allowed_placeholders)
        .run(text)
        .2
}

/// 提取文本中出现的占位符 token（按首次出现顺序去重）。
pub fn find_placeholders(text: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for m in re_poison().find_iter(text) {
        let token = m.as_str().to_string();
        if !seen.contains(&token) {
            seen.push(token);
        }
    }
    seen
}

/// 占位符完整性校验：文本与映射表双向对账，空 Vec = 通过。
pub fn check_placeholder_integrity(
    text: &str,
    placeholder_map: &BTreeMap<String, String>,
) -> Vec<PlaceholderProblem> {
    let present = find_placeholders(text);
    let mut problems: Vec<PlaceholderProblem> = present
        .iter()
        .filter(|t| !placeholder_map.contains_key(*t))
        .map(|t| PlaceholderProblem::UnknownInText(t.clone()))
        .collect();
    for key in placeholder_map.keys() {
        if !present.contains(key) {
            problems.push(PlaceholderProblem::MissingFromText(key.clone()));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 3, 21).unwrap()
    }

    fn opts() -> RedactionOptions {
        RedactionOptions {
            strict_luhn: false,
            rewrite_absolute_time: true,
            now: Some(now()),
        }
    }

    fn hit_count(report: &RedactionReport, rule: &str) -> usize {
        report
            .hits
            .iter()
            .find(|h| h.rule_id == rule)
            .map(|h| h.count)
            .unwrap_or(0)
    }

    fn words(pairs: &[(&str, Option<&str>)]) -> Vec<CustomWord> {
        pairs
            .iter()
            .map(|(o, r)| CustomWord {
                original: o.to_string(),
                replacement: r.map(|s| s.to_string()),
            })
            .collect()
    }

    #[test]
    fn phone_hit_and_boundaries() {
        let report = redact("电话13800138000，备用15912345678。", &opts(), &[]);
        assert_eq!(report.text, "电话⟦PHONE-1⟧，备用⟦PHONE-2⟧。");
        assert_eq!(hit_count(&report, "phone"), 2);
        assert_eq!(
            report.placeholder_map.get("⟦PHONE-1⟧").map(String::as_str),
            Some("13800138000")
        );

        let miss = redact("编号1380013800（10位）和138001380001（12位）", &opts(), &[]);
        assert_eq!(hit_count(&miss, "phone"), 0);
        assert_eq!(hit_count(&miss, "card"), 0);

        let plus = redact("手机 +8613800138000 联系", &opts(), &[]);
        assert_eq!(plus.text, "手机 +⟦PHONE-1⟧ 联系");
    }

    #[test]
    fn phone_dedupe_reuses_placeholder() {
        let report = redact("13800138000 出现两次：13800138000", &opts(), &[]);
        assert_eq!(report.text, "⟦PHONE-1⟧ 出现两次：⟦PHONE-1⟧");
        assert_eq!(report.placeholder_map.len(), 1);
        assert_eq!(hit_count(&report, "phone"), 2);
    }

    #[test]
    fn email_wins_over_phone_inside_local_part() {
        let report = redact("邮箱 13800138000@126.com 收", &opts(), &[]);
        assert_eq!(report.text, "邮箱 ⟦EMAIL-1⟧ 收");
        assert_eq!(hit_count(&report, "email"), 1);
        assert_eq!(hit_count(&report, "phone"), 0);
    }

    #[test]
    fn email_basic_and_miss() {
        let report = redact("联系 bob.smith+tag@example.co.uk 或 a@b", &opts(), &[]);
        assert_eq!(report.text, "联系 ⟦EMAIL-1⟧ 或 a@b");
        assert_eq!(hit_count(&report, "email"), 1);
    }

    #[test]
    fn id_card_classification() {
        let report = redact(
            "身份证11010519491231002X，旧证110105491231002，卡6222020200112233",
            &opts(),
            &[],
        );
        assert_eq!(report.text, "身份证⟦ID-1⟧，旧证⟦ID-2⟧，卡⟦CARD-1⟧");
        assert_eq!(hit_count(&report, "id"), 2);
        assert_eq!(hit_count(&report, "card"), 1);
    }

    #[test]
    fn id18_invalid_date_falls_to_card() {
        let report = redact("怪号999999199913999999", &opts(), &[]);
        assert_eq!(report.text, "怪号⟦CARD-1⟧");
        assert_eq!(hit_count(&report, "id"), 0);
        assert_eq!(hit_count(&report, "card"), 1);
    }

    #[test]
    fn id17_plus_x_shape_redacted_as_id() {
        let report = redact("残缺12345678901234567X形状", &opts(), &[]);
        assert_eq!(report.text, "残缺⟦ID-1⟧形状");
        assert_eq!(hit_count(&report, "id"), 1);
    }

    #[test]
    fn card_strict_luhn_tightening() {
        let lenient = RedactionOptions {
            strict_luhn: false,
            ..opts()
        };
        let strict = RedactionOptions {
            strict_luhn: true,
            ..opts()
        };
        let lenient_report = redact("卡号1234567890123", &lenient, &[]);
        assert_eq!(lenient_report.text, "卡号⟦CARD-1⟧");
        let strict_report = redact("卡号1234567890123", &strict, &[]);
        assert_eq!(strict_report.text, "卡号1234567890123");
        assert_eq!(hit_count(&strict_report, "card"), 0);

        let valid = redact("维萨4111111111111111", &strict, &[]);
        assert_eq!(valid.text, "维萨⟦CARD-1⟧");
    }

    #[test]
    fn card_run_length_bounds() {
        let report = redact("12位123456789012 和20位12345678901234567890", &opts(), &[]);
        assert_eq!(report.text, "12位123456789012 和20位12345678901234567890");
        assert_eq!(hit_count(&report, "card"), 0);
    }

    #[test]
    fn path_mac_windows_tilde() {
        let report = redact(
            "mac /Users/zhangsan/Documents/secret.md，win C:\\Users\\zhang\\Desktop\\x.txt，家 ~/notes/priv.md，正斜杠 c:/Users/zhang/temp",
            &opts(),
            &[],
        );
        assert_eq!(
            report.text,
            "mac ⟦PATH-1⟧，win ⟦PATH-2⟧，家 ⟦PATH-3⟧，正斜杠 ⟦PATH-4⟧"
        );
        assert_eq!(hit_count(&report, "path"), 4);
    }

    #[test]
    fn path_stops_at_cjk_punctuation() {
        let report = redact("路径/Users/zhang/发票。后续文字", &opts(), &[]);
        assert_eq!(report.text, "路径⟦PATH-1⟧。后续文字");
    }

    #[test]
    fn url_secret_values_only() {
        let report = redact(
            "https://api.example.com/v1/data?token=abc123&page=2&sign=s-9&key=k-9",
            &opts(),
            &[],
        );
        assert_eq!(
            report.text,
            "https://api.example.com/v1/data?token=⟦URLSECRET-1⟧&page=2&sign=⟦URLSECRET-2⟧&key=⟦URLSECRET-3⟧"
        );
        assert_eq!(hit_count(&report, "urlsecret"), 3);
    }

    #[test]
    fn url_secret_compound_param_names() {
        let report = redact("https://x.com/cb?access_token=zz&session_key=kk&foo=bar", &opts(), &[]);
        assert_eq!(
            report.text,
            "https://x.com/cb?access_token=⟦URLSECRET-1⟧&session_key=⟦URLSECRET-2⟧&foo=bar"
        );
    }

    #[test]
    fn time_rewrite_variants() {
        let report = redact(
            "会议 2025-03-14 开的，2025年3月19日复查，2025/03/14 08:30:00 签到，未来 2025-04-21 截止",
            &opts(),
            &[],
        );
        assert_eq!(
            report.text,
            "会议 上周五 开的，前天复查，上周五 签到，未来 1个月后 截止"
        );
        assert_eq!(hit_count(&report, "time"), 4);
    }

    #[test]
    fn time_invalid_or_mixed_separators_skipped() {
        let report = redact("非法 2025-13-01，混合 2025-03/14", &opts(), &[]);
        assert_eq!(report.text, "非法 2025-13-01，混合 2025-03/14");
        assert_eq!(hit_count(&report, "time"), 0);
    }

    #[test]
    fn time_disabled_without_now() {
        let options = RedactionOptions {
            rewrite_absolute_time: true,
            now: None,
            ..opts()
        };
        let report = redact("2025-03-14", &options, &[]);
        assert_eq!(report.text, "2025-03-14");
    }

    #[test]
    fn frontmatter_stripped_wholesale() {
        let input = "---\ntitle: 私密\ncontact: 13800138000\n---\n正文15912345678";
        let report = redact(input, &opts(), &[]);
        assert_eq!(report.text, "正文⟦PHONE-1⟧");
        assert_eq!(hit_count(&report, "frontmatter"), 1);
        assert_eq!(hit_count(&report, "phone"), 1);
    }

    #[test]
    fn custom_words_explicit_and_auto() {
        let dict = words(&[("张三", Some("张工")), ("李四", None)]);
        let report = redact("张三和李四开会", &opts(), &dict);
        assert_eq!(report.text, "张工和⟦WORD-1⟧开会");
        assert_eq!(hit_count(&report, "word"), 2);
        assert_eq!(
            report.placeholder_map.get("⟦WORD-1⟧").map(String::as_str),
            Some("李四")
        );
    }

    #[test]
    fn custom_words_longest_first() {
        let dict = words(&[("张三", None), ("张三丰", None)]);
        let report = redact("张三丰来访，张三也在", &opts(), &dict);
        assert_eq!(report.text, "⟦WORD-1⟧来访，⟦WORD-2⟧也在");
        assert_eq!(
            report.placeholder_map.get("⟦WORD-1⟧").map(String::as_str),
            Some("张三丰")
        );
    }

    #[test]
    fn poison_placeholder_neutralized_no_collision() {
        let input = "已有⟦PHONE-7⟧伪造，真实13800138000在此";
        let report = redact(input, &opts(), &[]);
        assert_eq!(report.text, "已有⟦PLACEHOLDER-1⟧伪造，真实⟦PHONE-1⟧在此");
        assert_eq!(hit_count(&report, "placeholder"), 1);
        assert_eq!(
            report.placeholder_map.get("⟦PLACEHOLDER-1⟧").map(String::as_str),
            Some("⟦PHONE-7⟧")
        );
    }

    #[test]
    fn word_guard_protects_placeholder_interior() {
        let dict = words(&[("ACE", None)]);
        let input = "⟦PHONE-1⟧ 与 ACE 变量";
        let report = redact(input, &opts(), &dict);
        assert_eq!(report.text, "⟦PLACEHOLDER-1⟧ 与 ⟦WORD-1⟧ 变量");
        assert!(!report.text.contains("PLACEH⟦"));
    }

    #[test]
    fn adversarial_kitchen_sink() {
        let dict = words(&[("王五", Some("客户甲"))]);
        let input = "---\nauthor: 王五\n---\n王五（13800138000，bob@x.com）从 /Users/wangwu/税单.docx 导出，接口 https://api.x.com/e?token=t0&n=1，证件11010519491231002X，会议 2025-03-14";
        let report = redact(input, &opts(), &dict);
        assert_eq!(
            report.text,
            "客户甲（⟦PHONE-1⟧，⟦EMAIL-1⟧）从 ⟦PATH-1⟧ 导出，接口 https://api.x.com/e?token=⟦URLSECRET-1⟧&n=1，证件⟦ID-1⟧，会议 上周五"
        );
        assert_eq!(hit_count(&report, "frontmatter"), 1);
        assert_eq!(hit_count(&report, "word"), 1);
        assert_eq!(hit_count(&report, "phone"), 1);
        assert_eq!(hit_count(&report, "email"), 1);
        assert_eq!(hit_count(&report, "path"), 1);
        assert_eq!(hit_count(&report, "urlsecret"), 1);
        assert_eq!(hit_count(&report, "id"), 1);
        assert_eq!(hit_count(&report, "time"), 1);
        assert_eq!(report.rule_set_version, RULE_CATALOG_VERSION);
    }

    #[test]
    fn scan_consistent_with_redact() {
        let dict = words(&[("王五", None)]);
        let input = "---\na: 1\n---\n王五 13800138000 bob@x.com /Users/w/b.docx 2025-03-14 ⟦PHONE-9⟧";
        let scanned = scan(input, &opts(), &dict);
        let report = redact(input, &opts(), &dict);
        assert_eq!(scanned, report.hits);
    }

    #[test]
    fn scan_with_map_exempts_legitimate_placeholders() {
        let dict = words(&[("王五", None)]);
        let report = redact("王五 13800138000", &opts(), &dict);
        let raw_hits = scan_with_map(&report.text, &opts(), &dict, &BTreeMap::new());
        assert!(raw_hits.iter().any(|h| h.rule_id == "placeholder"));
        let exempt_hits = scan_with_map(&report.text, &opts(), &dict, &report.placeholder_map);
        assert!(!exempt_hits.iter().any(|h| h.rule_id == "placeholder"));
        assert!(exempt_hits.is_empty());
    }

    #[test]
    fn placeholder_integrity_both_directions() {
        let report = redact("13800138000 与 bob@x.com", &opts(), &[]);
        assert!(check_placeholder_integrity(&report.text, &report.placeholder_map).is_empty());

        let tampered = format!("{} 拼出 ⟦PHONE-88⟧", report.text);
        let problems = check_placeholder_integrity(&tampered, &report.placeholder_map);
        assert_eq!(
            problems,
            vec![PlaceholderProblem::UnknownInText("⟦PHONE-88⟧".to_string())]
        );

        let dropped = report.text.replace("⟦EMAIL-1⟧", "某邮箱");
        let problems = check_placeholder_integrity(&dropped, &report.placeholder_map);
        assert_eq!(
            problems,
            vec![PlaceholderProblem::MissingFromText("⟦EMAIL-1⟧".to_string())]
        );
    }

    #[test]
    fn find_placeholders_ordered_distinct() {
        let text = "⟦PHONE-2⟧ ⟦PHONE-1⟧ ⟦PHONE-2⟧";
        assert_eq!(
            find_placeholders(text),
            vec!["⟦PHONE-2⟧".to_string(), "⟦PHONE-1⟧".to_string()]
        );
    }

    #[test]
    fn luhn_unit() {
        assert!(luhn_valid("4111111111111111"));
        assert!(luhn_valid("79927398713"));
        assert!(!luhn_valid("4111111111111112"));
        assert!(!luhn_valid("1234567890123"));
        assert!(!luhn_valid(""));
    }

    #[test]
    fn split_frontmatter_boundaries() {
        let (fm, body) = split_frontmatter("---\nkey: v\n---\n# T\n");
        assert_eq!(fm, Some("---\nkey: v\n---\n"));
        assert_eq!(body, "# T\n");

        let (fm, body) = split_frontmatter("# 无头\n");
        assert_eq!(fm, None);
        assert_eq!(body, "# 无头\n");

        let (fm, body) = split_frontmatter("\u{FEFF}---\nk: 1\n---\n正文");
        assert!(fm.is_some());
        assert_eq!(body, "正文");

        let (fm, _) = split_frontmatter("---\n未闭合\n");
        assert_eq!(fm, None);
    }

    #[test]
    fn parse_word_line_variants() {
        assert_eq!(
            parse_word_line("  张三=>张工  "),
            Some(CustomWord {
                original: "张三".to_string(),
                replacement: Some("张工".to_string())
            })
        );
        assert_eq!(
            parse_word_line("李四"),
            Some(CustomWord {
                original: "李四".to_string(),
                replacement: None
            })
        );
        assert_eq!(parse_word_line("  "), None);
        assert_eq!(
            parse_word_line("空=>"),
            Some(CustomWord {
                original: "空".to_string(),
                replacement: None
            })
        );
    }

    #[test]
    fn empty_input_and_no_rules() {
        let report = redact("", &opts(), &[]);
        assert_eq!(report.text, "");
        assert!(report.hits.is_empty());
        assert!(report.placeholder_map.is_empty());
    }

    #[test]
    fn phone_adjacent_cjk_punctuation_hits() {
        let report = redact("【13800138000】", &opts(), &[]);
        assert_eq!(report.text, "【⟦PHONE-1⟧】");
    }
}
