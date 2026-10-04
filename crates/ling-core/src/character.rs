//! 角色（Character）、身份（Identity）与特质（Trait）的数据与规则。
//!
//! 纯逻辑模块：不做任何 IO，时间以 Unix 秒时间戳注入，保证可单测。
//!
//! 特质活跃度机制：
//! - 每个特质带「活跃度」（0.0 ~ 1.0），刻画该性格当前在角色身上的鲜明程度；
//! - 活跃度随时间按半衰期指数衰减（久不被提及的性格逐渐沉寂）；
//! - 长期未被触及且活跃度低于阈值的特质标记为「休眠」（dormant）；
//! - 休眠特质再次被提及时「唤醒」，活跃度回涌；
//! - 对话中涌现的新特质自动写入角色档案（同名视为再次被提及，自动去重合并）。

use std::cmp::Ordering;

/// 新特质诞生时的初始活跃度
pub const NEW_TRAIT_ACTIVITY: f32 = 0.8;
/// 活跃度衰减半衰期（天）：每经过这么多天活跃度减半
pub const DECAY_HALF_LIFE_DAYS: f64 = 3.0;
/// 活跃度低于该阈值视为「沉寂」
pub const DORMANT_ACTIVITY_THRESHOLD: f32 = 0.15;
/// 连续这么多天未被触及，才允许进入休眠
pub const DORMANT_AFTER_DAYS: f64 = 7.0;
/// 唤醒时活跃度回涌的下限
pub const AWAKEN_ACTIVITY_FLOOR: f32 = 0.6;
/// 每次被提及带来的活跃度增量
pub const TOUCH_ACTIVITY_GAIN: f32 = 0.4;
/// 一天的秒数
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// 身份：角色「是谁」的静态档案
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// 姓名
    pub name: String,
    /// 称号
    pub title: String,
    /// 出身
    pub origin: String,
    /// 所属阵营（或领地）
    pub faction: String,
}

impl Identity {
    pub fn new(name: &str, title: &str, origin: &str, faction: &str) -> Self {
        Self {
            name: name.trim().to_string(),
            title: title.trim().to_string(),
            origin: origin.trim().to_string(),
            faction: faction.trim().to_string(),
        }
    }
}

/// 特质：角色身上一条可演化的性格/能力标签
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterTrait {
    /// 特质名（保留展示原样）
    pub name: String,
    /// 描述（随对话涌现而追加）
    pub description: String,
    /// 活跃度 0.0 ~ 1.0
    pub activity: f32,
    /// 最近一次被触及的 Unix 时间戳（秒）
    pub last_touched: i64,
    /// 是否休眠
    pub dormant: bool,
}

impl CharacterTrait {
    /// 新建特质：初始活跃度为 [`NEW_TRAIT_ACTIVITY`]
    pub fn new(name: &str, description: &str, now: i64) -> Self {
        Self {
            name: name.trim().to_string(),
            description: description.trim().to_string(),
            activity: NEW_TRAIT_ACTIVITY,
            last_touched: now,
            dormant: false,
        }
    }

    /// 名字归一化（去空白 + 小写），用于同名匹配
    fn normalized_name(&self) -> String {
        self.name.trim().to_lowercase()
    }

    /// 距上次被触及的天数（不足一天按小数计）
    pub fn days_since_touched(&self, now: i64) -> f64 {
        (now - self.last_touched).max(0) as f64 / SECONDS_PER_DAY
    }

    /// 计算衰减后的活跃度（不改状态）：activity × 0.5^(elapsed / 半衰期)
    pub fn decayed_activity(&self, now: i64) -> f32 {
        let half_lives = (self.days_since_touched(now) / DECAY_HALF_LIFE_DAYS) as f32;
        self.activity * 0.5f32.powf(half_lives)
    }

    /// 推进到时刻 `now`：应用衰减，并做休眠判定（长期未触及 + 活跃度沉寂）
    pub fn advance_to(&mut self, now: i64) {
        let decayed = self.decayed_activity(now);
        let days = self.days_since_touched(now);
        self.activity = decayed.clamp(0.0, 1.0);
        if !self.dormant && days >= DORMANT_AFTER_DAYS && decayed < DORMANT_ACTIVITY_THRESHOLD {
            self.dormant = true;
        }
    }

    /// 被提及：唤醒（解除休眠）并让活跃度回涌
    pub fn touch(&mut self, now: i64) {
        self.last_touched = now;
        self.dormant = false;
        self.activity = (self.activity + TOUCH_ACTIVITY_GAIN).clamp(AWAKEN_ACTIVITY_FLOOR, 1.0);
    }

    pub fn is_dormant(&self) -> bool {
        self.dormant
    }
}

/// 角色：身份档案 + 特质集合
#[derive(Debug, Clone)]
pub struct Character {
    pub id: i64,
    pub identity: Identity,
    pub traits: Vec<CharacterTrait>,
    /// 建档时间（Unix 秒）
    pub created_at: i64,
}

impl Character {
    pub fn new(id: i64, identity: Identity, now: i64) -> Self {
        Self {
            id,
            identity,
            traits: Vec::new(),
            created_at: now,
        }
    }

    /// 按名字查找特质（忽略大小写与首尾空白）
    pub fn find_trait(&self, name: &str) -> Option<&CharacterTrait> {
        let key = name.trim().to_lowercase();
        self.traits.iter().find(|t| t.normalized_name() == key)
    }

    fn find_trait_mut(&mut self, name: &str) -> Option<&mut CharacterTrait> {
        let key = name.trim().to_lowercase();
        self.traits.iter_mut().find(|t| t.normalized_name() == key)
    }

    /// 对话中涌现的特质自动写入：
    /// - 已存在同名特质 → 视为一次提及（唤醒/回涌），并按需追加描述；
    /// - 不存在 → 新建特质。
    pub fn emerge_trait(&mut self, name: &str, description: &str, now: i64) {
        let trimmed_desc = description.trim();
        match self.find_trait_mut(name) {
            Some(trait_) => {
                if !trimmed_desc.is_empty() && !trait_.description.contains(trimmed_desc) {
                    if trait_.description.is_empty() {
                        trait_.description = trimmed_desc.to_string();
                    } else {
                        trait_.description = format!("{}；{}", trait_.description, trimmed_desc);
                    }
                }
                trait_.touch(now);
            }
            None => self.traits.push(CharacterTrait::new(name, description, now)),
        }
    }

    /// 提及一组特质名：命中的特质被唤醒/回涌，返回命中数量
    pub fn mention_traits(&mut self, names: &[&str], now: i64) -> usize {
        let mut hits = 0;
        for name in names {
            if let Some(trait_) = self.find_trait_mut(name) {
                trait_.touch(now);
                hits += 1;
            }
        }
        hits
    }

    /// 把对话中提及但尚未收录的特质名自动写入档案
    pub fn absorb_mentions(&mut self, names: &[&str], now: i64) {
        for name in names {
            self.emerge_trait(name, "", now);
        }
    }

    /// 推进所有特质的时间演化（衰减 + 休眠判定）
    pub fn advance_to(&mut self, now: i64) {
        for trait_ in &mut self.traits {
            trait_.advance_to(now);
        }
    }

    /// 未休眠的特质，按活跃度降序
    pub fn active_traits(&self) -> Vec<&CharacterTrait> {
        let mut list: Vec<&CharacterTrait> = self.traits.iter().filter(|t| !t.dormant).collect();
        list.sort_by(|a, b| b.activity.partial_cmp(&a.activity).unwrap_or(Ordering::Equal));
        list
    }

    /// 已休眠的特质
    pub fn dormant_traits(&self) -> Vec<&CharacterTrait> {
        self.traits.iter().filter(|t| t.dormant).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_700_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn new_trait_has_sane_initial_state() {
        let t = CharacterTrait::new("谨慎", "凡事三思", T0);
        assert!((t.activity - NEW_TRAIT_ACTIVITY).abs() < 1e-6);
        assert!(!t.dormant);
        assert_eq!(t.last_touched, T0);
        assert_eq!(t.days_since_touched(T0), 0.0);
    }

    #[test]
    fn activity_halves_every_half_life() {
        let t = CharacterTrait::new("谨慎", "", T0);
        // 恰好经过一个半衰期（3 天）→ 活跃度减半
        let later = T0 + (DECAY_HALF_LIFE_DAYS as i64) * DAY;
        assert!((t.decayed_activity(later) - NEW_TRAIT_ACTIVITY / 2.0).abs() < 1e-3);
    }

    #[test]
    fn dormant_requires_long_untouched_and_low_activity() {
        let mut t = CharacterTrait::new("谨慎", "", T0);
        t.activity = 0.1;
        // 只过了 3 天：活跃度虽已沉寂，但时间不足 → 不休眠
        t.advance_to(T0 + 3 * DAY);
        assert!(!t.dormant);
        // 累计 8 天未触及 → 休眠
        t.advance_to(T0 + 8 * DAY);
        assert!(t.dormant);
    }

    #[test]
    fn touching_awakens_and_boosts_activity() {
        let mut t = CharacterTrait::new("谨慎", "", T0);
        t.advance_to(T0 + 30 * DAY);
        assert!(t.dormant);
        let t1 = T0 + 31 * DAY;
        t.touch(t1);
        assert!(!t.dormant);
        assert!(t.activity >= AWAKEN_ACTIVITY_FLOOR - 1e-6);
        assert!(t.activity <= 1.0 + 1e-6);
        assert_eq!(t.last_touched, t1);
    }

    #[test]
    fn emerge_trait_records_new_and_merges_existing() {
        let mut c = Character::new(1, Identity::new("玄铃", "守夜人", "北境", "霜环"), T0);
        c.emerge_trait("谨慎", "凡事三思", T0);
        assert_eq!(c.traits.len(), 1);
        // 对话中再次涌现同名特质：去重合并 + 视为一次触及
        let t1 = T0 + DAY;
        c.emerge_trait("谨慎", "夜里加倍警惕", t1);
        assert_eq!(c.traits.len(), 1);
        assert!(c.traits[0].description.contains("凡事三思"));
        assert!(c.traits[0].description.contains("夜里加倍警惕"));
        assert_eq!(c.traits[0].last_touched, t1);
        // 不同名 → 新增
        c.emerge_trait("坚韧", "", t1);
        assert_eq!(c.traits.len(), 2);
    }

    #[test]
    fn mention_matches_case_insensitive_and_absorbs_new() {
        let mut c = Character::new(1, Identity::new("甲", "", "", ""), T0);
        c.emerge_trait("brave", "", T0);
        assert_eq!(c.mention_traits(&["Brave", "未知特质"], T0 + DAY), 1);
        // 未命中的提及自动写入档案
        c.absorb_mentions(&["机敏"], T0 + DAY);
        assert!(c.find_trait("机敏").is_some());
        let absorbed = c.find_trait("机敏").unwrap();
        assert!((absorbed.activity - NEW_TRAIT_ACTIVITY).abs() < 1e-6);
    }

    #[test]
    fn advance_marks_dormant_and_filters_lists() {
        let mut c = Character::new(1, Identity::new("甲", "", "", ""), T0);
        c.emerge_trait("热情", "", T0);
        c.emerge_trait("沉稳", "", T0);
        // 「热情」长期未被提及而休眠；「沉稳」前一天刚被提及，保持活跃
        c.mention_traits(&["沉稳"], T0 + 29 * DAY);
        c.advance_to(T0 + 30 * DAY);
        assert_eq!(c.dormant_traits().len(), 1);
        assert_eq!(c.active_traits().len(), 1);
        assert_eq!(c.active_traits()[0].name, "沉稳");
    }
}
