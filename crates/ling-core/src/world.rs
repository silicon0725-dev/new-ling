//! 领地与世界最小状态。
//!
//! 时间流速：现实 1 秒 = 世界 72 秒（1:72），即现实一天 = 世界 72 天，
//! 世界的一天只对应现实 20 分钟。
//! 四季与昼夜目前只保留数据结构与最简派生逻辑，更细的规则见各 TODO。

/// 四季
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    /// 春
    Spring,
    /// 夏
    Summer,
    /// 秋
    Autumn,
    /// 冬
    Winter,
}

impl Season {
    /// 中文名
    pub fn chinese_name(&self) -> &'static str {
        match self {
            Season::Spring => "春",
            Season::Summer => "夏",
            Season::Autumn => "秋",
            Season::Winter => "冬",
        }
    }
}

/// 昼夜阶段（按世界时粗划分）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayPhase {
    /// 黎明
    Dawn,
    /// 白昼
    Day,
    /// 黄昏
    Dusk,
    /// 夜晚
    Night,
}

impl DayPhase {
    /// 中文名
    pub fn chinese_name(&self) -> &'static str {
        match self {
            DayPhase::Dawn => "黎明",
            DayPhase::Day => "白昼",
            DayPhase::Dusk => "黄昏",
            DayPhase::Night => "夜晚",
        }
    }
}

/// 领地：世界中的一块可归属疆域
#[derive(Debug, Clone)]
pub struct Territory {
    pub id: i64,
    pub name: String,
    /// 归属角色 id；None 表示无主之地
    pub owner_character_id: Option<i64>,
    /// 稳定度 0.0 ~ 1.0
    pub stability: f32,
}

impl Territory {
    pub fn new(id: i64, name: &str, owner_character_id: Option<i64>, stability: f32) -> Self {
        Self {
            id,
            name: name.to_string(),
            owner_character_id,
            stability: stability.clamp(0.0, 1.0),
        }
    }
}

/// 世界时钟：只保存流逝秒数与流速参数，派生量按需计算
#[derive(Debug, Clone)]
pub struct WorldClock {
    /// 世界内已流逝的总秒数
    pub elapsed_seconds: i64,
    /// 时间流速：每个现实秒对应的世界秒数（1:72）
    pub seconds_per_real_second: i64,
    /// 一个世界日的秒数
    pub seconds_per_day: i64,
    /// 一个季节的世界日数（四季等分）
    pub days_per_season: i64,
}

impl Default for WorldClock {
    fn default() -> Self {
        Self {
            elapsed_seconds: 0,
            // 1:72 —— 现实一天 = 世界 72 天
            seconds_per_real_second: 72,
            seconds_per_day: 86_400,
            days_per_season: 90,
        }
    }
}

impl WorldClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推进若干现实秒（按 1:72 折算成世界秒）
    pub fn advance_real_seconds(&mut self, real_seconds: i64) {
        self.elapsed_seconds += real_seconds * self.seconds_per_real_second;
    }

    /// 当前是第几个世界日（从 0 计）
    pub fn day_index(&self) -> i64 {
        self.elapsed_seconds.div_euclid(self.seconds_per_day)
    }

    /// 今日已流逝的秒数
    pub fn seconds_of_day(&self) -> i64 {
        self.elapsed_seconds.rem_euclid(self.seconds_per_day)
    }

    /// 当前季节（四季等分）
    // TODO(世界): 昼夜长短随季节变化（夏长冬短）后，改为非等分历法
    pub fn season(&self) -> Season {
        match self
            .day_index()
            .div_euclid(self.days_per_season)
            .rem_euclid(4)
        {
            0 => Season::Spring,
            1 => Season::Summer,
            2 => Season::Autumn,
            _ => Season::Winter,
        }
    }

    /// 当前昼夜阶段
    // TODO(世界): 接入精细历法后按季节修正黎明/黄昏的时刻区间
    pub fn day_phase(&self) -> DayPhase {
        let hour = self.seconds_of_day() / 3600;
        match hour {
            5..=7 => DayPhase::Dawn,
            8..=16 => DayPhase::Day,
            17..=19 => DayPhase::Dusk,
            _ => DayPhase::Night,
        }
    }
}

/// 世界：名称 + 时钟 + 领地集合
#[derive(Debug, Clone)]
pub struct World {
    pub name: String,
    pub clock: WorldClock,
    pub territories: Vec<Territory>,
}

impl World {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            clock: WorldClock::new(),
            territories: Vec::new(),
        }
    }

    pub fn add_territory(&mut self, territory: Territory) {
        self.territories.push(territory);
    }

    /// 某角色名下的领地
    pub fn territories_of(&self, character_id: i64) -> Vec<&Territory> {
        self.territories
            .iter()
            .filter(|t| t.owner_character_id == Some(character_id))
            .collect()
    }

    // TODO(世界): 资源产出、人口与稳定度随时间/事件演化的规则。
    // TODO(世界): 世界事件（天灾、商队、战争）与三层记忆、角色弧光的联动。
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_flows_at_one_to_seventy_two() {
        let mut clock = WorldClock::new();
        assert_eq!(clock.seconds_per_real_second, 72);
        clock.advance_real_seconds(100);
        assert_eq!(clock.elapsed_seconds, 7_200);
        // 现实一天 = 世界 72 天
        clock.advance_real_seconds(86_400);
        assert_eq!(clock.day_index(), 72);
        // 世界的一天 = 现实 20 分钟
        let mut fresh = WorldClock::new();
        fresh.advance_real_seconds(1_200);
        assert_eq!(fresh.seconds_of_day(), 0);
        assert_eq!(fresh.day_index(), 1);
    }

    #[test]
    fn seasons_and_phases_derive_from_elapsed_time() {
        let mut clock = WorldClock::new();
        // 第 91 个世界日的清晨 6 点 → 夏 / 黎明（每季 90 天）
        clock.elapsed_seconds = 90 * 86_400 + 6 * 3600;
        assert_eq!(clock.season(), Season::Summer);
        assert_eq!(clock.day_phase(), DayPhase::Dawn);
        // 深夜 23 点
        clock.elapsed_seconds = 90 * 86_400 + 23 * 3600;
        assert_eq!(clock.day_phase(), DayPhase::Night);
    }

    #[test]
    fn territories_filter_by_owner() {
        let mut world = World::new("霜环之地");
        world.add_territory(Territory::new(1, "北境", Some(7), 0.8));
        world.add_territory(Territory::new(2, "荒原", None, 0.3));
        assert_eq!(world.territories_of(7).len(), 1);
        assert_eq!(world.territories_of(9).len(), 0);
        assert_eq!(world.territories.len(), 2);
    }
}
