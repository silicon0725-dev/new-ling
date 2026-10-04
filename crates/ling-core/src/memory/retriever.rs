//! 记忆检索接口与关键词实现。

use std::collections::HashSet;

/// 相似度检索接口。
/// 当前实现为关键词/简单相似度；后续接入向量检索时，
/// 只需提供新的实现（如 embedding 余弦相似度），引擎与上层代码无需改动。
pub trait Retriever: Send + Sync {
    /// 计算 query 与 text 的相关性得分，取值 [0.0, 1.0]，越大越相关。
    fn similarity(&self, query: &str, text: &str) -> f32;
}

/// 关键词检索器：分词后按 Jaccard 词重合率打分。
///
/// 分词规则：
/// - 连续的 ASCII 字母/数字（不区分大小写）视为一个词；
/// - 中日韩表意字符逐字拆分为单字 token（适配中文无空格的书写习惯）；
/// - 其余字符一律作为分隔符。
#[derive(Debug, Clone, Copy, Default)]
pub struct KeywordRetriever;

/// 判断是否为 CJK 表意字符（简化处理，覆盖常用汉字与假名区段）
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'   // 扩展 A 区
        | '\u{4E00}'..='\u{9FFF}' // 基本区（常用汉字）
        | '\u{F900}'..='\u{FAFF}' // 兼容表意区
        | '\u{3040}'..='\u{30FF}' // 平假名 / 片假名
    )
}

impl KeywordRetriever {
    /// 简易分词：ASCII 词 + CJK 单字
    pub fn tokenize(text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut word = String::new();
        for ch in text.chars() {
            if ch.is_ascii_alphanumeric() {
                word.push(ch.to_ascii_lowercase());
            } else {
                if !word.is_empty() {
                    tokens.push(std::mem::take(&mut word));
                }
                if is_cjk(ch) {
                    tokens.push(ch.to_string());
                }
            }
        }
        if !word.is_empty() {
            tokens.push(word);
        }
        tokens
    }
}

impl Retriever for KeywordRetriever {
    fn similarity(&self, query: &str, text: &str) -> f32 {
        let a: HashSet<String> = Self::tokenize(query).into_iter().collect();
        let b: HashSet<String> = Self::tokenize(text).into_iter().collect();
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let intersection = a.intersection(&b).count();
        let union = a.union(&b).count();
        intersection as f32 / union as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_ascii_words_and_cjk_chars() {
        let tokens = KeywordRetriever::tokenize("星图 Layout v2，收敛！");
        // 中文标点「，」「！」是分隔符，不产 token
        assert_eq!(tokens, vec!["星", "图", "layout", "v2", "收", "敛"]);
    }

    #[test]
    fn similarity_jaccard_bounds() {
        let r = KeywordRetriever;
        assert!((r.similarity("星图布局", "星图布局") - 1.0).abs() < 1e-6);
        assert_eq!(r.similarity("星图", "领地"), 0.0);
        assert_eq!(r.similarity("", "任意"), 0.0);
        // {星,图,布,局} vs {星,图,渲,染}：交集 2 / 并集 6
        assert!((r.similarity("星图布局", "星图渲染") - 2.0 / 6.0).abs() < 1e-6);
    }
}
