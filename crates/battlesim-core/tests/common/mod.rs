//! Shared stat blocks and a small scripted proposer for the scenario tests.
#![allow(dead_code)]

use battlesim_core::state::Creature;
use battlesim_core::{Choice, Combatant, Constraint, Engine, Expect, Id, Input, Setup, StatBlock, trace};
use std::sync::Arc;

pub fn block(yaml: &str) -> Arc<StatBlock> {
    Arc::new(StatBlock::from_yaml(yaml).unwrap_or_else(|e| panic!("stat block did not load: {e}")))
}

pub const ROGUE: &str = r#"
name: Rogue 5
ac: 15
hp: 33
death_saves: true
abilities: {dex: 18}
resources:
  sneak_attack: {max: 1, recharge: any_turn}
actions:
  - name: Rapier
    tags: [finesse]
    target: melee
    effect:
      - attack:
          bonus: 7
          on_hit:
            - damage: {dice: 1d8+4, type: piercing}
traits:
  - name: Sneak Attack
    after: Hit
    as: attacker
    when: "has_tag(action, finesse) and (event.advantage or (ally_engaged_with(target) and not event.disadvantage))"
    uses: {sneak_attack: 1}
    effect:
      - damage: {dice: 3d6, type: inherit}
"#;

pub const FIGHTER: &str = r#"
name: Fighter 5
ac: 18
hp: 44
death_saves: true
abilities: {str: 16, con: 14}
actions:
  - name: Longsword
    target: melee
    effect:
      - attack: {bonus: 6, on_hit: [{damage: {dice: 1d8+3, type: slashing}}]}
  - name: Second Wind
    cost: bonus_action
    uses: {second_wind: 1}
    target: self
    effect:
      - heal: {dice: 1d10+5}
resources:
  second_wind: {max: 1, recharge: short_rest}
"#;

pub const WIZARD: &str = r#"
name: Wizard 5
ac: 15
hp: 27
death_saves: true
resources:
  spell_slot_1: {max: 4, recharge: long_rest}
  spell_slot_3: {max: 2, recharge: long_rest}
actions:
  - name: Fire Bolt
    tags: [spell]
    target: ranged
    effect:
      - attack: {bonus: 7, on_hit: [{damage: {dice: 2d10, type: fire}}]}
  - name: Fireball
    uses: {spell_slot_3: 1}
    tags: [spell]
    target: ranged
    approx: Hits one creature. Areas are not modeled yet.
    effect:
      - save:
          ability: dex
          dc: 15
          on_fail: [{damage: {dice: 8d6, type: fire}}]
          on_success: [{damage: {dice: 8d6, type: fire, half: true}}]
  - name: Counterspell
    cost: reaction
    uses: {spell_slot_3: 1}
    tags: [spell]
    effect:
      - save:
          who: event.actor
          ability: con
          dc: 15
          on_fail:
            - cancel: event
traits:
  - name: Shield
    before: Hit
    as: target
    optional: true
    uses: {reaction: 1, spell_slot_1: 1}
    effect:
      - apply:
          name: Shield
          duration: {until: start_of_next_turn}
          traits:
            - {before: Ac, as: self, change: {bonus: 5}}
  - name: Counterspell
    before: ActionUsed
    as: any
    when: "is_enemy(event.actor) and has_tag(event.action, spell)"
    optional: true
    effect:
      - use: {action: Counterspell, as_action: true}
"#;

/// A caster with Fireball and nothing to respond with.
pub const EVOKER: &str = r#"
name: Evoker
ac: 12
hp: 27
death_saves: true
resources:
  spell_slot_3: {max: 2, recharge: long_rest}
actions:
  - name: Fireball
    uses: {spell_slot_3: 1}
    tags: [spell]
    target: ranged
    effect:
      - save:
          ability: dex
          dc: 15
          on_fail: [{damage: {dice: 8d6, type: fire}}]
          on_success: [{damage: {dice: 8d6, type: fire, half: true}}]
"#;

/// A caster whose only trick is Counterspell, as the 2024 rules write it.
pub const ABJURER: &str = r#"
name: Abjurer
ac: 12
hp: 40
resources:
  spell_slot_3: {max: 2, recharge: long_rest}
actions:
  - name: Dagger
    target: melee
    effect:
      - attack: {bonus: 4, on_hit: [{damage: {dice: 1d4+2, type: piercing}}]}
  - name: Counterspell
    cost: reaction
    uses: {spell_slot_3: 1}
    tags: [spell]
    effect:
      - save:
          who: event.actor
          ability: con
          dc: 15
          on_fail:
            - cancel: event
traits:
  - name: Counterspell
    before: ActionUsed
    as: any
    when: "is_enemy(event.actor) and has_tag(event.action, spell)"
    optional: true
    effect:
      - use: {action: Counterspell, as_action: true}
"#;

pub const GNOLL: &str = r#"
name: Gnoll
ac: 15
hp: 20
actions:
  - name: Spear
    target: melee
    effect:
      - attack: {bonus: 4, on_hit: [{damage: {dice: 1d6+2, type: piercing}}]}
"#;

pub const OGRE: &str = r#"
name: Ogre
size: large
ac: 11
hp: 59
abilities: {str: 19, dex: 8, con: 16}
actions:
  - name: Greatclub
    target: melee
    effect:
      - attack: {bonus: 6, on_hit: [{damage: {dice: 2d8+4, type: bludgeoning}}]}
"#;

pub const WOLF: &str = r#"
name: Wolf
ac: 12
hp: 11
abilities: {dex: 15}
actions:
  - name: Bite
    target: melee
    approx: Prone ends by itself at the end of the wolf's next turn. Standing up takes movement, which is not modeled yet.
    effect:
      - attack:
          bonus: 4
          on_hit:
            - damage: {dice: 1d6+2, type: piercing}
            - if:
                when: "target.size <= large"
                then:
                  - apply: {condition: prone, duration: {until: end_of_next_turn}}
traits:
  - name: Pack Tactics
    before: Attack
    as: attacker
    when: "ally_engaged_with(target)"
    change: {advantage: true}
"#;

/// A fight under a script: every input is given by hand, and each helper
/// first checks that the engine is asking for that kind of input.
pub struct Fight {
    pub engine: Engine,
}

impl Fight {
    /// Starts the fight and answers the initiative rolls, one per combatant
    /// in setup order.
    pub fn start(setup: Setup, initiative: &[u32]) -> Fight {
        let engine = Engine::new(&setup).unwrap_or_else(|e| panic!("setup failed: {e}"));
        let mut fight = Fight { engine };
        for roll in initiative {
            fight.roll("1d20", *roll);
        }
        fight
    }

    pub fn duel(a: (&str, &str), b: (&str, &str), initiative: &[u32]) -> Fight {
        let setup = Setup {
            combatants: vec![Combatant::new(a.0, "party", &block(a.1)), Combatant::new(b.0, "monsters", &block(b.1))],
            ..Setup::default()
        };
        Fight::start(setup, initiative)
    }

    pub fn id(&self, name: &str) -> Id {
        let found = self.engine.state().creatures.iter().find(|c| c.name == name);
        found.unwrap_or_else(|| panic!("no creature named {name}")).id
    }

    pub fn creature(&self, name: &str) -> &Creature {
        self.engine.state().creature(self.id(name))
    }

    pub fn pool(&self, name: &str, resource: &str) -> u32 {
        self.creature(name).resource(resource)
    }

    pub fn roll(&mut self, spec: &str, value: u32) -> &mut Self {
        match self.engine.expects() {
            Expect::Roll { spec: wanted, .. } => assert_eq!(wanted.to_string(), spec, "wrong roll requested"),
            other => panic!("expected a {spec} roll, but the engine wants {other:?}\n{}", self.trace()),
        }
        self.engine.push(Input::Rolled(value)).unwrap();
        self
    }

    fn expect_choice(&self, who: &str) -> Constraint {
        match self.engine.expects() {
            Expect::Choice { actor, constraint } => {
                assert_eq!(self.engine.state().name(*actor), who, "wrong creature is choosing\n{}", self.trace());
                constraint.clone()
            }
            other => panic!("expected a choice by {who}, but the engine wants {other:?}\n{}", self.trace()),
        }
    }

    pub fn act(&mut self, who: &str, action: &str, target: Option<&str>) -> &mut Self {
        assert_eq!(self.expect_choice(who), Constraint::Step);
        let targets = target.map(|t| self.id(t)).into_iter().collect();
        let choice = Choice::Act { action: action.to_string(), targets };
        self.engine.push(Input::Chose(choice)).unwrap_or_else(|e| panic!("{e}\n{}", self.trace()));
        self
    }

    pub fn end_turn(&mut self, who: &str) -> &mut Self {
        assert_eq!(self.expect_choice(who), Constraint::Step);
        self.engine.push(Input::Chose(Choice::EndTurn)).unwrap();
        self
    }

    pub fn react(&mut self, who: &str, rule: &str, fire: bool) -> &mut Self {
        match self.expect_choice(who) {
            Constraint::React { rule: asked, .. } => assert_eq!(asked, rule),
            other => panic!("expected a reaction prompt, found {other:?}"),
        }
        self.engine.push(Input::Chose(Choice::React(fire))).unwrap();
        self
    }

    pub fn target(&mut self, who: &str, target: &str) -> &mut Self {
        assert!(matches!(self.expect_choice(who), Constraint::Target { .. }));
        self.engine.push(Input::Chose(Choice::Target(self.id(target)))).unwrap();
        self
    }

    pub fn mark(&self) -> usize {
        self.engine.log().len()
    }

    pub fn trace(&self) -> String {
        trace::render(self.engine.log(), self.engine.state())
    }

    /// The trace from log position `mark` on.
    pub fn trace_from(&self, mark: usize) -> String {
        let log = self.engine.log();
        trace::render_entries(&log.entries()[mark..], log, self.engine.state())
    }

    /// Every logged event as one line, without ids or indentation.
    pub fn lines(&self) -> Vec<String> {
        self.engine.log().events().map(|e| trace::describe(e, self.engine.state())).collect()
    }

    pub fn has_line(&self, line: &str) -> bool {
        self.lines().iter().any(|l| l == line)
    }
}
