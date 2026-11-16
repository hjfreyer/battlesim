//! The data model: stat blocks, actions, traits and effect nodes.
//!
//! Everything here is plain data that loads from YAML or JSON. A player
//! character and a monster are the same type. Nothing in this module runs;
//! the engine interprets it.

use crate::dice::Dice;
use crate::expr::Expr;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

/// Pools every creature has without declaring them.
pub const ACTION: &str = "action";
pub const BONUS_ACTION: &str = "bonus_action";
pub const REACTION: &str = "reaction";
/// Death saves are tracked as what is left: three successes to stabilize,
/// three failures to die.
pub const DEATH_SUCCESSES: &str = "death_successes";
pub const DEATH_FAILURES: &str = "death_failures";

const BUILT_IN_POOLS: &[(&str, u32)] =
    &[(ACTION, 1), (BONUS_ACTION, 1), (REACTION, 1), (DEATH_SUCCESSES, 3), (DEATH_FAILURES, 3)];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Tiny,
    Small,
    #[default]
    Medium,
    Large,
    Huge,
    Gargantuan,
}

impl Size {
    pub const NAMES: &[&str] = &["tiny", "small", "medium", "large", "huge", "gargantuan"];
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ability {
    Str,
    Dex,
    Con,
    Int,
    Wis,
    Cha,
}

impl Ability {
    pub fn name(self) -> &'static str {
        match self {
            Ability::Str => "str",
            Ability::Dex => "dex",
            Ability::Con => "con",
            Ability::Int => "int",
            Ability::Wis => "wis",
            Ability::Cha => "cha",
        }
    }
}

fn ten() -> u32 {
    10
}

/// Ability scores. Anything left out is 10.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abilities {
    #[serde(default = "ten")]
    pub str: u32,
    #[serde(default = "ten")]
    pub dex: u32,
    #[serde(default = "ten")]
    pub con: u32,
    #[serde(default = "ten")]
    pub int: u32,
    #[serde(default = "ten")]
    pub wis: u32,
    #[serde(default = "ten")]
    pub cha: u32,
}

impl Default for Abilities {
    fn default() -> Self {
        Abilities { str: 10, dex: 10, con: 10, int: 10, wis: 10, cha: 10 }
    }
}

impl Abilities {
    pub fn score(&self, ability: Ability) -> u32 {
        match ability {
            Ability::Str => self.str,
            Ability::Dex => self.dex,
            Ability::Con => self.con,
            Ability::Int => self.int,
            Ability::Wis => self.wis,
            Ability::Cha => self.cha,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defenses {
    #[serde(default)]
    pub resist: Vec<String>,
    #[serde(default)]
    pub immune: Vec<String>,
    #[serde(default)]
    pub vulnerable: Vec<String>,
    #[serde(default)]
    pub condition_immune: Vec<String>,
}

/// When a spent resource comes back.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recharge {
    /// At the start of every turn, anyone's. For once-per-turn features.
    AnyTurn,
    /// At the start of the creature's own turn.
    OwnTurn,
    /// At the start of the creature's own turn on a d6 roll of this or more.
    Roll(u32),
    ShortRest,
    LongRest,
    #[default]
    Never,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDef {
    pub max: u32,
    #[serde(default)]
    pub recharge: Recharge,
}

/// What an action costs to declare.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cost {
    #[default]
    Action,
    BonusAction,
    Reaction,
    Free,
}

impl Cost {
    pub fn pool(self) -> Option<&'static str> {
        match self {
            Cost::Action => Some(ACTION),
            Cost::BonusAction => Some(BONUS_ACTION),
            Cost::Reaction => Some(REACTION),
            Cost::Free => None,
        }
    }
}

/// Who an action can be aimed at.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRule {
    /// No target. The effect runs once.
    #[default]
    None,
    /// The creature using the action.
    #[serde(rename = "self")]
    Me,
    /// An enemy the creature can reach: one in the enemy front line, or
    /// anyone when no enemy is left standing in front.
    Melee,
    /// Any enemy.
    Ranged,
    /// Any creature on the same side, the user included.
    Ally,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    pub name: String,
    #[serde(default)]
    pub cost: Cost,
    /// Resources spent when the action takes effect, by pool name.
    #[serde(default)]
    pub uses: BTreeMap<String, u32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub target: TargetRule,
    #[serde(default)]
    pub effect: Vec<Node>,
    /// The rules text this entry was written from.
    #[serde(default)]
    pub text: Option<String>,
    /// What this entry leaves out or simplifies.
    #[serde(default)]
    pub approx: Option<String>,
}

impl ActionDef {
    /// The damage type of the first damage this action deals, which is what
    /// extra damage "of the weapon's type" inherits.
    pub fn damage_type(&self) -> Option<&str> {
        fn find(nodes: &[Node]) -> Option<&str> {
            nodes.iter().find_map(|n| match n {
                Node::Damage(d) if d.kind != INHERIT => Some(d.kind.as_str()),
                Node::Attack(a) => find(&a.on_hit).or_else(|| find(&a.on_miss)),
                Node::Save(s) => find(&s.on_fail).or_else(|| find(&s.on_success)),
                Node::If(i) => find(&i.then).or_else(|| find(&i.otherwise)),
                _ => None,
            })
        }
        find(&self.effect)
    }
}

/// A damage `type` that takes the type of whatever triggered it.
pub const INHERIT: &str = "inherit";

/// One step of an effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Node {
    Attack(Arc<AttackNode>),
    Save(Arc<SaveNode>),
    Damage(DamageNode),
    Heal(AmountNode),
    TempHp(AmountNode),
    Apply(Arc<EffectDef>),
    Restore(RestoreNode),
    If(IfNode),
    Use(UseNode),
    Cancel(CancelTarget),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttackNode {
    pub bonus: i32,
    #[serde(default)]
    pub on_hit: Vec<Node>,
    #[serde(default)]
    pub on_miss: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveNode {
    /// Who saves. Defaults to the effect's target.
    #[serde(default)]
    pub who: Option<Expr>,
    pub ability: Ability,
    pub dc: u32,
    #[serde(default)]
    pub on_fail: Vec<Node>,
    #[serde(default)]
    pub on_success: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DamageNode {
    pub dice: Dice,
    #[serde(rename = "type")]
    pub kind: String,
    /// Deal half, rounded down. For the success branch of a save.
    #[serde(default)]
    pub half: bool,
    /// Who takes it. Defaults to the effect's target.
    #[serde(default)]
    pub to: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmountNode {
    pub dice: Dice,
    #[serde(default)]
    pub to: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreNode {
    pub resource: String,
    /// How much comes back. Left out, the pool refills.
    #[serde(default)]
    pub amount: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IfNode {
    pub when: Expr,
    #[serde(default)]
    pub then: Vec<Node>,
    #[serde(default, rename = "else")]
    pub otherwise: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UseNode {
    pub action: String,
    /// False runs the named action's effect as part of this one, asking for
    /// a target and charging nothing, which is how Multiattack works. True
    /// uses it as an action in its own right: it pays its cost, and other
    /// creatures can respond to it.
    #[serde(default)]
    pub as_action: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelTarget {
    /// The pending event the enclosing rule or reaction responded to.
    Event,
}

/// Something attached to a creature for a while: a condition, a buff, a
/// spell's lingering part. It carries rules, which is all it does.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDef {
    #[serde(default)]
    pub name: Option<String>,
    /// A built-in condition whose rules this effect carries.
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub duration: Duration,
    #[serde(default)]
    pub traits: Vec<TraitDef>,
    /// Who it attaches to. Defaults to the effect's target.
    #[serde(default)]
    pub to: Option<Expr>,
}

impl EffectDef {
    pub fn condition(name: &str) -> EffectDef {
        EffectDef { condition: Some(name.to_string()), ..EffectDef::default() }
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().or(self.condition.as_deref()).unwrap_or("effect")
    }

    /// Every rule this effect carries: its own, then its condition's.
    pub fn rules(&self) -> impl Iterator<Item = &TraitDef> {
        let library = self.condition.as_ref().and_then(|c| conditions().get(c));
        self.traits.iter().chain(library.into_iter().flatten())
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Duration {
    /// Until the fight ends or something removes it.
    #[default]
    Fight,
    Until(Until),
}

/// Turn boundaries an effect can end on. "Next turn" is counted for the
/// creature that applied the effect.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    StartOfNextTurn,
    EndOfNextTurn,
    /// The end of the turn in progress, whoever's it is.
    EndOfTurn,
}

/// The native event kinds. A trait names one of these to say what it reacts to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventKind {
    Damage,
    Heal,
    TempHp,
    ResourceSpent,
    ResourceRestored,
    EffectAttached,
    EffectRemoved,
    Moved,
    Engaged,
    Disengaged,
    Dropped,
    Died,
    ActionUsed,
    Attack,
    Hit,
    Miss,
    Save,
    SaveFailed,
    SaveSucceeded,
    DeathSave,
    FightStarted,
    RoundStarted,
    TurnStarted,
    TurnEnded,
    FightEnded,
    Fizzled,
    Rolled,
    Chose,
    Ac,
}

/// The part a trait's owner must play in an event for the trait to apply.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The creature the event is about: whoever attacks, acts, takes the
    /// damage, starts the turn.
    #[default]
    #[serde(rename = "self")]
    Me,
    Attacker,
    Actor,
    Target,
    Source,
    /// Any event of the kind, whoever is involved.
    Any,
}

/// Where a before-rule sits in the order. Lower bands go first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Adjust = 1,
    Resist = 2,
    Vulnerable = 3,
    React = 4,
}

/// A declarative edit to a pending event. Each field applies only to the
/// event kinds that have it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    #[serde(default)]
    pub advantage: Option<bool>,
    #[serde(default)]
    pub disadvantage: Option<bool>,
    /// Added to the event's bonus, or to the value of an `Ac` query.
    #[serde(default)]
    pub bonus: Option<i32>,
    #[serde(default)]
    pub crit: Option<bool>,
    #[serde(default)]
    pub auto_fail: Option<bool>,
    /// Replaces the amount of a `ResourceSpent`. Zero waives the cost.
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(default)]
    pub resist: Option<bool>,
    #[serde(default)]
    pub vulnerable: Option<bool>,
    #[serde(default)]
    pub immune: Option<bool>,
    /// Subtracted from damage before resistance.
    #[serde(default)]
    pub reduce: Option<u32>,
}

impl Change {
    fn set_fields(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut note = |set: bool, name| {
            if set {
                out.push(name)
            }
        };
        note(self.advantage.is_some(), "advantage");
        note(self.disadvantage.is_some(), "disadvantage");
        note(self.bonus.is_some(), "bonus");
        note(self.crit.is_some(), "crit");
        note(self.auto_fail.is_some(), "auto_fail");
        note(self.n.is_some(), "n");
        note(self.resist.is_some(), "resist");
        note(self.vulnerable.is_some(), "vulnerable");
        note(self.immune.is_some(), "immune");
        note(self.reduce.is_some(), "reduce");
        out
    }

    pub fn fields_for(kind: EventKind) -> &'static [&'static str] {
        match kind {
            EventKind::Attack => &["advantage", "disadvantage", "bonus"],
            EventKind::Save => &["advantage", "disadvantage", "bonus", "auto_fail"],
            EventKind::Hit => &["crit"],
            EventKind::Damage => &["resist", "vulnerable", "immune", "reduce"],
            EventKind::Ac => &["bonus"],
            EventKind::ResourceSpent => &["n"],
            _ => &[],
        }
    }
}

/// A rule written as data. It names an event kind and a moment, before or
/// after, and says what to do when one comes by.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraitDef {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub before: Option<EventKind>,
    #[serde(default)]
    pub after: Option<EventKind>,
    #[serde(default, rename = "as")]
    pub role: Role,
    #[serde(default)]
    pub when: Option<Expr>,
    /// Resources the rule spends each time it applies.
    #[serde(default)]
    pub uses: BTreeMap<String, u32>,
    /// True when the owner chooses whether to apply it. The choice counts as
    /// a reaction-style decision: an incapacitated owner is not asked.
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub band: Option<Band>,
    #[serde(default)]
    pub change: Option<Change>,
    #[serde(default)]
    pub cancel: bool,
    #[serde(default)]
    pub effect: Vec<Node>,
    /// Recorded but never applied. For abilities the model does not cover.
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub approx: Option<String>,
}

impl TraitDef {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("unnamed trait")
    }

    pub fn band(&self) -> Band {
        self.band.unwrap_or(if self.optional { Band::React } else { Band::Adjust })
    }

    /// True when applying the rule takes more than a verdict on the event:
    /// it spends something, runs an effect, or asks its owner first.
    pub fn has_body(&self) -> bool {
        self.optional || !self.uses.is_empty() || !self.effect.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatBlock {
    pub name: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub size: Size,
    #[serde(default)]
    pub abilities: Abilities,
    pub ac: u32,
    pub hp: u32,
    /// Saving throw bonuses that differ from the plain ability modifier.
    #[serde(default)]
    pub saves: BTreeMap<Ability, i32>,
    /// Initiative bonus. Defaults to the Dexterity modifier.
    #[serde(default)]
    pub initiative: Option<i32>,
    /// True for creatures that fall unconscious at 0 hit points and make
    /// death saves. Others die at 0.
    #[serde(default)]
    pub death_saves: bool,
    #[serde(default)]
    pub defenses: Defenses,
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceDef>,
    #[serde(default)]
    pub actions: Vec<ActionDef>,
    #[serde(default)]
    pub traits: Vec<TraitDef>,
}

impl StatBlock {
    /// Loads and checks one stat block. JSON is YAML, so this reads both.
    pub fn from_yaml(text: &str) -> Result<StatBlock, String> {
        let value: serde_json::Value = serde_saphyr::from_str(text).map_err(|e| e.to_string())?;
        let block: StatBlock = serde_json::from_value(value).map_err(|e| e.to_string())?;
        block.validate()?;
        Ok(block)
    }

    pub fn action(&self, name: &str) -> Option<&ActionDef> {
        self.actions.iter().find(|a| a.name == name)
    }

    pub fn modifier(&self, ability: Ability) -> i32 {
        (self.abilities.score(ability) as i32 - 10).div_euclid(2)
    }

    pub fn save_bonus(&self, ability: Ability) -> i32 {
        self.saves.get(&ability).copied().unwrap_or_else(|| self.modifier(ability))
    }

    pub fn initiative_bonus(&self) -> i32 {
        self.initiative.unwrap_or_else(|| self.modifier(Ability::Dex))
    }

    /// The size of a pool, built-in or declared.
    pub fn max_resource(&self, name: &str) -> Option<u32> {
        if let Some(def) = self.resources.get(name) {
            return Some(def.max);
        }
        let built_in = BUILT_IN_POOLS.iter().find(|(n, _)| *n == name)?;
        let death = name == DEATH_SUCCESSES || name == DEATH_FAILURES;
        (!death || self.death_saves).then_some(built_in.1)
    }

    /// Every pool the creature starts with, full.
    pub fn starting_resources(&self) -> BTreeMap<String, u32> {
        let mut pools: BTreeMap<String, u32> =
            BUILT_IN_POOLS.iter().filter_map(|(name, _)| Some((name.to_string(), self.max_resource(name)?))).collect();
        pools.extend(self.resources.iter().map(|(name, def)| (name.clone(), def.max)));
        pools
    }

    /// Checks everything that can be checked without a fight: names that
    /// must resolve, fields that must fit the event they change.
    pub fn validate(&self) -> Result<(), String> {
        let fail = |what: String| format!("{}: {what}", self.name);
        for action in &self.actions {
            let at = format!("action `{}`", action.name);
            self.check_uses(&action.uses).map_err(|e| fail(format!("{at}: {e}")))?;
            self.check_nodes(&action.effect).map_err(|e| fail(format!("{at}: {e}")))?;
        }
        for t in &self.traits {
            let at = format!("trait `{}`", t.label());
            self.check_uses(&t.uses).map_err(|e| fail(format!("{at}: {e}")))?;
            self.check_trait(t).map_err(|e| fail(format!("{at}: {e}")))?;
        }
        Ok(())
    }

    fn check_uses(&self, uses: &BTreeMap<String, u32>) -> Result<(), String> {
        match uses.keys().find(|name| self.max_resource(name).is_none()) {
            Some(name) => Err(self.unknown_resource(name)),
            None => Ok(()),
        }
    }

    fn unknown_resource(&self, name: &str) -> String {
        let mut known: Vec<&str> = self.resources.keys().map(String::as_str).collect();
        known.extend(BUILT_IN_POOLS.iter().map(|(n, _)| *n).filter(|n| self.max_resource(n).is_some()));
        format!("unknown resource `{name}`; this stat block has {}", known.join(", "))
    }

    fn check_trait(&self, t: &TraitDef) -> Result<(), String> {
        let kind = match (t.before, t.after) {
            (Some(kind), None) | (None, Some(kind)) => kind,
            _ => return Err("needs exactly one of `before` and `after`".into()),
        };
        if t.after.is_some() && (t.cancel || t.change.is_some()) {
            return Err("`change` and `cancel` only make sense in a `before` rule".into());
        }
        if let Some(change) = &t.change {
            let allowed = Change::fields_for(kind);
            if let Some(field) = change.set_fields().into_iter().find(|f| !allowed.contains(f)) {
                return Err(format!(
                    "`change` cannot set `{field}` on {kind:?}; it can set {}",
                    if allowed.is_empty() { "nothing".to_string() } else { allowed.join(", ") }
                ));
            }
        }
        self.check_nodes(&t.effect)
    }

    fn check_nodes(&self, nodes: &[Node]) -> Result<(), String> {
        for node in nodes {
            match node {
                Node::Attack(a) => {
                    self.check_nodes(&a.on_hit)?;
                    self.check_nodes(&a.on_miss)?;
                }
                Node::Save(s) => {
                    self.check_nodes(&s.on_fail)?;
                    self.check_nodes(&s.on_success)?;
                }
                Node::If(i) => {
                    self.check_nodes(&i.then)?;
                    self.check_nodes(&i.otherwise)?;
                }
                Node::Apply(effect) => {
                    match (&effect.name, &effect.condition) {
                        (None, None) => return Err("`apply` needs a `name` or a `condition`".into()),
                        (_, Some(c)) if !conditions().contains_key(c) => {
                            let known: Vec<&str> = conditions().keys().map(String::as_str).collect();
                            return Err(format!("unknown condition `{c}`; known conditions are {}", known.join(", ")));
                        }
                        _ => {}
                    }
                    // An attached rule runs for whoever bears the effect, so
                    // the resources it names cannot be checked here.
                    for t in &effect.traits {
                        self.check_trait(t).map_err(|e| format!("effect `{}`: {e}", effect.label()))?;
                    }
                }
                Node::Restore(r) => {
                    if self.max_resource(&r.resource).is_none() {
                        return Err(self.unknown_resource(&r.resource));
                    }
                }
                Node::Use(u) => {
                    if self.action(&u.action).is_none() {
                        let known: Vec<&str> = self.actions.iter().map(|a| a.name.as_str()).collect();
                        return Err(format!(
                            "`use` names unknown action `{}`; actions are {}",
                            u.action,
                            known.join(", ")
                        ));
                    }
                }
                Node::Damage(_) | Node::Heal(_) | Node::TempHp(_) | Node::Cancel(_) => {}
            }
        }
        Ok(())
    }
}

/// The built-in conditions, each a list of rules.
pub fn conditions() -> &'static BTreeMap<String, Vec<TraitDef>> {
    static LIBRARY: LazyLock<BTreeMap<String, Vec<TraitDef>>> = LazyLock::new(|| {
        let text = include_str!("../data/conditions.yaml");
        let value: serde_json::Value = serde_saphyr::from_str(text).expect("conditions.yaml is valid YAML");
        serde_json::from_value(value).expect("conditions.yaml matches the trait schema")
    });
    &LIBRARY
}

/// Conditions under which a creature takes no actions or reactions.
pub const INCAPACITATING: &[&str] = &["incapacitated", "stunned", "paralyzed", "unconscious"];
