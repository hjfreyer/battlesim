//! Scenario tests for the core rules and the data-driven ones.

mod common;
use battlesim_core::{Choice, Combatant, Expect, Input, Invalid, Life, Setup, StatBlock};
use common::*;

fn party_vs(party: &[(&str, &str)], monsters: &[(&str, &str)]) -> Setup {
    let side = |members: &[(&str, &str)], side: &str| -> Vec<Combatant> {
        members.iter().map(|(name, yaml)| Combatant::new(name, side, &block(yaml))).collect()
    };
    Setup { combatants: [side(party, "party"), side(monsters, "monsters")].concat(), ..Setup::default() }
}

#[test]
fn pack_tactics_gives_advantage_once_an_ally_is_engaged() {
    let setup = party_vs(&[("Fighter", FIGHTER)], &[("Grey", WOLF), ("Red", WOLF)]);
    let mut fight = Fight::start(setup, &[1, 20, 15]);
    // Nobody is in melee with the fighter yet, so the first wolf rolls flat.
    fight.act("Grey", "Bite", Some("Fighter")).roll("1d20", 2);
    assert!(fight.has_line("Engaged(Grey, Fighter)"));
    // The second wolf has a packmate on the target.
    fight.act("Red", "Bite", Some("Fighter")).roll("2d20 keep highest", 19).roll("1d6+2", 5);
    assert!(fight.has_line("EffectAttached(Fighter, prone)"));
    // A prone fighter swings with disadvantage.
    fight.act("Fighter", "Longsword", Some("Grey")).roll("2d20 keep lowest", 3);
    fight.end_turn("Fighter");
    // Prone lasts until the end of the biting wolf's next turn, and while it
    // lasts a melee attacker has advantage whether or not it has the pack.
    fight.end_turn("Grey");
    assert!(fight.creature("Fighter").has_condition("prone"));
    fight.end_turn("Red");
    assert!(!fight.creature("Fighter").has_condition("prone"));
    assert!(fight.has_line("EffectRemoved(Fighter, prone)"));
}

/// The ogre drops the fighter on the first swing. The rogue is there so the
/// fight goes on.
fn fighter_down() -> Fight {
    let mut setup = party_vs(&[("Fighter", FIGHTER), ("Rogue", ROGUE)], &[("Ogre", OGRE)]);
    setup.combatants[0].hp = Some(5);
    let mut fight = Fight::start(setup, &[10, 5, 20]);
    fight.act("Ogre", "Greatclub", Some("Fighter")).roll("1d20", 15).roll("2d8+4", 10);
    assert_eq!(fight.creature("Fighter").life, Life::Down);
    assert!(fight.has_line("Dropped(Fighter)"));
    assert!(fight.has_line("EffectAttached(Fighter, unconscious)"));
    fight
}

#[test]
fn a_natural_20_on_a_death_save_gets_the_creature_up() {
    let mut fight = fighter_down();
    fight.roll("1d20", 20);
    let fighter = fight.creature("Fighter");
    assert_eq!((fighter.life, fighter.hp), (Life::Alive, 1));
    assert!(!fighter.has_condition("unconscious"));
    // Awake on its own turn, so it gets to act.
    fight.act("Fighter", "Longsword", Some("Ogre"));
}

#[test]
fn three_failed_death_saves_kill() {
    let mut fight = fighter_down();
    fight.roll("1d20", 1); // a natural 1 is two failures
    assert_eq!(fight.pool("Fighter", "death_failures"), 1);
    fight.end_turn("Rogue").end_turn("Ogre");
    fight.roll("1d20", 9);
    assert_eq!(fight.creature("Fighter").life, Life::Dead);
    assert!(fight.has_line("Died(Fighter)"));
    assert!(!fight.engine.state().ended);
}

#[test]
fn three_successful_death_saves_stabilize() {
    let mut fight = fighter_down();
    for _ in 0..3 {
        fight.roll("1d20", 10);
        fight.end_turn("Rogue").end_turn("Ogre");
    }
    assert!(fight.creature("Fighter").is_stable());
    // A stable creature makes no more death saves: the next thing the
    // engine wants is the rogue's turn again.
    fight.end_turn("Rogue");
    assert_eq!(fight.lines().iter().filter(|l| *l == "DeathSave(Fighter)").count(), 3);
}

#[test]
fn damage_while_down_is_a_failed_death_save_and_a_melee_hit_is_a_crit() {
    let mut fight = fighter_down();
    fight.roll("1d20", 10);
    fight.end_turn("Rogue");
    // Attacks against an unconscious creature have advantage, a melee hit is
    // a critical hit, and a critical hit while down is two failures.
    fight.act("Ogre", "Greatclub", Some("Fighter")).roll("2d20 keep highest", 12).roll("4d8+4", 20);
    assert!(fight.lines().iter().any(|l| l.starts_with("Hit(") && l.ends_with("critical)")));
    assert_eq!(fight.pool("Fighter", "death_failures"), 1);
    assert_eq!(fight.pool("Fighter", "death_successes"), 2);
}

#[test]
fn healing_a_downed_creature_wakes_it_and_clears_its_death_saves() {
    let cleric = r#"
name: Cleric
ac: 16
hp: 30
death_saves: true
actions:
  - name: Cure Wounds
    target: ally
    effect:
      - heal: {dice: 2d8+3}
"#;
    let mut setup = party_vs(&[("Fighter", FIGHTER), ("Cleric", cleric)], &[("Ogre", OGRE)]);
    setup.combatants[0].hp = Some(5);
    let mut fight = Fight::start(setup, &[10, 5, 20]);
    fight.act("Ogre", "Greatclub", Some("Fighter")).roll("1d20", 15).roll("2d8+4", 10);
    fight.roll("1d20", 4);
    assert_eq!(fight.pool("Fighter", "death_failures"), 2);
    fight.act("Cleric", "Cure Wounds", Some("Fighter")).roll("2d8+3", 12);
    let fighter = fight.creature("Fighter");
    assert_eq!((fighter.life, fighter.hp), (Life::Alive, 12));
    assert_eq!(fight.pool("Fighter", "death_failures"), 3);
    assert!(!fighter.has_condition("unconscious"));
}

#[test]
fn monsters_die_at_zero_and_the_fight_ends_when_one_side_is_left() {
    let mut setup = party_vs(&[("Fighter", FIGHTER)], &[("Gnoll", GNOLL)]);
    setup.combatants[1].hp = Some(3);
    let mut fight = Fight::start(setup, &[20, 1]);
    fight.act("Fighter", "Longsword", Some("Gnoll")).roll("1d20", 15).roll("1d8+3", 6);
    assert!(fight.has_line("Died(Gnoll)"));
    assert!(!fight.has_line("Dropped(Gnoll)"));
    assert!(fight.has_line("FightEnded(party)"));
    assert_eq!(fight.engine.expects(), &Expect::Done);
    assert_eq!(fight.engine.state().winner.as_deref(), Some("party"));
    assert_eq!(fight.engine.push(Input::Rolled(3)), Err(Invalid::NothingExpected));
}

#[test]
fn a_party_with_everyone_down_has_lost() {
    let mut setup = party_vs(&[("Fighter", FIGHTER)], &[("Ogre", OGRE)]);
    setup.combatants[0].hp = Some(5);
    let mut fight = Fight::start(setup, &[1, 20]);
    fight.act("Ogre", "Greatclub", Some("Fighter")).roll("1d20", 15).roll("2d8+4", 10);
    assert!(fight.has_line("FightEnded(monsters)"));
    assert_eq!(fight.creature("Fighter").life, Life::Down);
}

#[test]
fn a_natural_1_misses_and_a_natural_20_crits() {
    let mut fight = Fight::duel(("Fighter", FIGHTER), ("Gnoll", GNOLL), &[20, 1]);
    fight.act("Fighter", "Longsword", Some("Gnoll")).roll("1d20", 20).roll("2d8+3", 11);
    assert!(fight.has_line("Hit(#8, total 26, AC 15, critical)"));
    fight.end_turn("Fighter");
    // 1 + 4 would miss AC 18 anyway; what matters is that no damage is asked for.
    fight.act("Gnoll", "Spear", Some("Fighter")).roll("1d20", 1);
    assert_eq!(fight.creature("Fighter").hp, 44);
}

const DEFENSES: &str = r#"
name: Warded
ac: 10
hp: 100
defenses:
  resist: [fire]
  immune: [poison]
  vulnerable: [cold]
  condition_immune: [poisoned]
"#;

const TESTER: &str = r#"
name: Tester
ac: 10
hp: 10
actions:
  - {name: Fire, cost: free, target: ranged, effect: [{damage: {dice: 11, type: fire}}]}
  - {name: Poison, cost: free, target: ranged, effect: [{damage: {dice: 11, type: poison}}]}
  - {name: Cold, cost: free, target: ranged, effect: [{damage: {dice: 11, type: cold}}]}
  - {name: Force, cost: free, target: ranged, effect: [{damage: {dice: 11, type: force}}]}
  - {name: Sicken, cost: free, target: ranged, effect: [{apply: {condition: poisoned}}]}
  - {name: Trip, cost: free, target: ranged, effect: [{apply: {condition: prone}}]}
"#;

#[test]
fn resistance_immunity_and_vulnerability() {
    let mut fight = Fight::duel(("Tester", TESTER), ("Warded", DEFENSES), &[20, 1]);
    for action in ["Fire", "Poison", "Cold", "Force"] {
        fight.act("Tester", action, Some("Warded"));
    }
    // A fixed amount needs no roll, so nothing was asked for along the way.
    for line in [
        "Damage(Warded, 5 fire, resisted)",
        "Damage(Warded, 0 poison, immune)",
        "Damage(Warded, 22 cold, vulnerable)",
        "Damage(Warded, 11 force)",
    ] {
        assert!(fight.has_line(line), "missing {line}\n{}", fight.trace());
    }
    assert_eq!(fight.creature("Warded").hp, 100 - 5 - 22 - 11);
}

#[test]
fn condition_immunity_cancels_the_effect() {
    let mut fight = Fight::duel(("Tester", TESTER), ("Warded", DEFENSES), &[20, 1]);
    fight.act("Tester", "Sicken", Some("Warded")).act("Tester", "Trip", Some("Warded"));
    assert!(fight.has_line("Fizzled(EffectAttached(Warded, poisoned), cancelled by a rule)"));
    assert!(fight.has_line("EffectAttached(Warded, prone)"));
    assert!(!fight.creature("Warded").has_condition("poisoned"));
}

#[test]
fn ranged_attacks_have_disadvantage_with_an_enemy_in_melee() {
    let mut setup = party_vs(&[("Wizard", WIZARD)], &[("Ogre", OGRE)]);
    let mut fight = Fight::start(setup.clone(), &[20, 1]);
    fight.act("Wizard", "Fire Bolt", Some("Ogre")).roll("1d20", 10);

    setup.engaged = vec![(0, 1)];
    let mut fight = Fight::start(setup, &[20, 1]);
    fight.act("Wizard", "Fire Bolt", Some("Ogre")).roll("2d20 keep lowest", 10);
}

#[test]
fn the_front_line_screens_the_back_line_from_melee() {
    let mut setup = party_vs(&[("Fighter", FIGHTER), ("Wizard", WIZARD)], &[("Ogre", OGRE)]);
    setup.combatants[0].hp = Some(5);
    setup.combatants[1].line = battlesim_core::Line::Back;
    let mut fight = Fight::start(setup, &[10, 5, 20]);
    let swing =
        |fight: &Fight, target: &str| Choice::Act { action: "Greatclub".into(), targets: vec![fight.id(target)] };
    assert_eq!(fight.engine.options(), vec![swing(&fight, "Fighter"), Choice::EndTurn]);
    let reach_past = Input::Chose(swing(&fight, "Wizard"));
    assert_eq!(fight.engine.push(reach_past), Err(Invalid::NotAllowed));

    // With the fighter down nobody is standing in front, so the wizard can
    // be reached. The fighter is still a legal target too.
    fight.act("Ogre", "Greatclub", Some("Fighter")).roll("1d20", 15).roll("2d8+4", 10);
    fight.roll("1d20", 12).end_turn("Wizard");
    assert_eq!(fight.engine.options(), vec![swing(&fight, "Fighter"), swing(&fight, "Wizard"), Choice::EndTurn]);
}

const BEAR: &str = r#"
name: Brown Bear
size: large
ac: 11
hp: 34
resources:
  roar: {max: 1, recharge: {roll: 5}}
actions:
  - name: Multiattack
    effect:
      - use: {action: Bite}
      - use: {action: Claw}
  - name: Bite
    target: melee
    effect:
      - attack: {bonus: 6, on_hit: [{damage: {dice: 1d8+4, type: piercing}}]}
  - name: Claw
    target: melee
    effect:
      - attack: {bonus: 6, on_hit: [{damage: {dice: 2d6+4, type: slashing}}]}
  - name: Roar
    cost: bonus_action
    uses: {roar: 1}
    target: ranged
    effect:
      - save: {ability: wis, dc: 12, on_fail: [{apply: {condition: poisoned, duration: {until: end_of_turn}}}]}
"#;

#[test]
fn multiattack_aims_each_part_separately_and_pays_once() {
    let setup = party_vs(&[("Fighter", FIGHTER), ("Rogue", ROGUE)], &[("Bear", BEAR)]);
    let mut fight = Fight::start(setup, &[1, 1, 20]);
    fight.act("Bear", "Multiattack", None);
    fight.target("Bear", "Fighter").roll("1d20", 2);
    fight.target("Bear", "Rogue").roll("1d20", 14).roll("2d6+4", 9);
    assert_eq!(fight.creature("Rogue").hp, 24);
    let lines = fight.lines();
    assert_eq!(lines.iter().filter(|l| l.starts_with("ActionUsed(")).count(), 1);
    assert_eq!(lines.iter().filter(|l| l.starts_with("ResourceSpent(Bear, action")).count(), 1);
    assert!(fight.has_line("Attack(Bear, Fighter, Bite, +6)"));
    assert!(fight.has_line("Attack(Bear, Rogue, Claw, +6)"));
}

#[test]
fn a_choice_with_one_option_is_taken_without_asking() {
    let mut fight = Fight::duel(("Fighter", FIGHTER), ("Bear", BEAR), &[1, 20]);
    fight.act("Bear", "Multiattack", None);
    // One enemy, so neither attack asks who to aim at.
    fight.roll("1d20", 2).roll("1d20", 3);
    assert!(fight.has_line("Chose(Bear, target, Bite at Fighter)"));
    // Forced choices are in the log but are not inputs.
    let inputs = fight.engine.log().inputs();
    assert_eq!(inputs.iter().filter(|i| matches!(i, Input::Chose(_))).count(), 1);
}

#[test]
fn a_resource_that_recharges_on_a_roll() {
    let mut fight = Fight::duel(("Fighter", FIGHTER), ("Bear", BEAR), &[1, 20]);
    fight.act("Bear", "Roar", Some("Fighter")).roll("1d20", 3);
    assert!(fight.creature("Fighter").has_condition("poisoned"));
    fight.end_turn("Bear");
    // "Until the end of the turn" meant the bear's turn.
    assert!(!fight.creature("Fighter").has_condition("poisoned"));
    fight.end_turn("Fighter");
    fight.roll("1d6", 4);
    assert_eq!(fight.pool("Bear", "roar"), 0);
    fight.end_turn("Bear").end_turn("Fighter");
    fight.roll("1d6", 5);
    assert_eq!(fight.pool("Bear", "roar"), 1);
    // Full again, so the next turn does not ask.
    fight.end_turn("Bear").end_turn("Fighter");
    fight.end_turn("Bear");
}

#[test]
fn an_optional_rule_can_pay_to_change_a_pending_event() {
    let dodger = format!(
        "{ROGUE}
  - name: Uncanny Dodge
    before: Damage
    as: target
    optional: true
    uses: {{reaction: 1}}
    change: {{resist: true}}
"
    );
    let mut fight = Fight::duel(("Rogue", &dodger), ("Ogre", OGRE), &[1, 20]);
    fight.act("Ogre", "Greatclub", Some("Rogue")).roll("1d20", 15).roll("2d8+4", 14);
    fight.react("Rogue", "Uncanny Dodge", true);
    assert!(fight.has_line("Damage(Rogue, 7 bludgeoning, resisted)"));
    assert!(fight.has_line("ResourceSpent(Rogue, reaction, 1)"));
    assert_eq!(fight.creature("Rogue").hp, 26);
}

#[test]
fn a_rule_waives_a_cost_by_changing_its_amount_to_zero() {
    let frugal = format!(
        "{WIZARD}
  - name: Free Shield
    before: ResourceSpent
    when: \"event.resource == spell_slot_1\"
    change: {{n: 0}}
"
    );
    let mut fight = Fight::duel(("Wizard", &frugal), ("Ogre", OGRE), &[1, 20]);
    fight.act("Ogre", "Greatclub", Some("Wizard")).roll("1d20", 11);
    fight.react("Wizard", "Shield", true);
    assert!(fight.has_line("ResourceSpent(Wizard, spell_slot_1, 0)"));
    // The cost was waived, not cancelled, so what it paid for still happened.
    assert!(fight.has_line("EffectAttached(Wizard, Shield)"));
    assert_eq!(fight.pool("Wizard", "spell_slot_1"), 4);
}

#[test]
fn a_stunned_creature_fails_dexterity_saves_and_loses_its_turn() {
    let mut setup = party_vs(&[("Alice", EVOKER)], &[("Gnoll", GNOLL)]);
    setup.combatants[1] = setup.combatants[1].clone().condition("stunned");
    setup.combatants[1].hp = None;
    let mut fight = Fight::start(setup, &[20, 1]);
    // No save roll: straight to the damage.
    fight.act("Alice", "Fireball", Some("Gnoll")).roll("8d6", 10);
    let lines = fight.lines();
    assert!(lines.iter().any(|l| l.starts_with("SaveFailed(")));
    // Alice is out of actions, so her turn ended without asking. The gnoll's
    // turn starts and ends with nothing in between, and nobody asks it.
    let start = lines.iter().position(|l| l == "TurnStarted(Gnoll)").unwrap();
    assert_eq!(lines[start + 1], "TurnEnded(Gnoll)");
    assert!(!lines.iter().any(|l| l.starts_with("Chose(Gnoll")));
}

#[test]
fn an_attack_at_a_target_that_already_died_fizzles() {
    let flurry = r#"
name: Duelist
ac: 15
hp: 30
actions:
  - name: Flurry
    target: melee
    effect:
      - attack: {bonus: 5, on_hit: [{damage: {dice: 9, type: slashing}}]}
      - attack: {bonus: 5, on_hit: [{damage: {dice: 9, type: slashing}}]}
"#;
    let mut setup = party_vs(&[("Duelist", flurry)], &[("Gnoll", GNOLL), ("Other gnoll", GNOLL)]);
    setup.combatants[1].hp = Some(4);
    let mut fight = Fight::start(setup, &[20, 5, 1]);
    fight.act("Duelist", "Flurry", Some("Gnoll")).roll("1d20", 18);
    assert!(fight.has_line("Died(Gnoll)"));
    assert!(fight.has_line("Fizzled(Attack(Duelist, Gnoll, Flurry, +5), its target is no longer in the fight)"));
    // The fizzle asked for no roll: the engine is already on the next turn.
    fight.end_turn("Other gnoll");
}

#[test]
fn stat_blocks_are_checked_when_they_load() {
    let cases = [
        ("name: X\nac: 10\nhp: 1\nactions:\n  - {name: A, uses: {ki: 1}}", "unknown resource `ki`"),
        ("name: X\nac: 10\nhp: 1\nactions:\n  - {name: A, effect: [{use: {action: B}}]}", "unknown action `B`"),
        (
            "name: X\nac: 10\nhp: 1\ntraits:\n  - {name: T, before: Damage, change: {advantage: true}}",
            "cannot set `advantage` on Damage",
        ),
        (
            "name: X\nac: 10\nhp: 1\ntraits:\n  - {name: T, before: Hit, after: Hit}",
            "exactly one of `before` and `after`",
        ),
        (
            "name: X\nac: 10\nhp: 1\ntraits:\n  - {name: T, after: Hit, cancel: true}",
            "only make sense in a `before` rule",
        ),
        (
            "name: X\nac: 10\nhp: 1\nactions:\n  - {name: A, effect: [{apply: {condition: dizzy}}]}",
            "unknown condition `dizzy`",
        ),
        (
            "name: X\nac: 10\nhp: 1\ntraits:\n  - {name: T, before: Hit, when: \"targt.size > 1\"}",
            "unknown name `targt`",
        ),
        ("name: X\nac: 10\nhp: 1\ntraits:\n  - {name: T, before: Smite}", "unknown variant `Smite`"),
        (
            "name: X\nac: 10\nhp: 1\nactions:\n  - {name: A, effect: [{damage: {dice: lots, type: fire}}]}",
            "not a dice expression",
        ),
        ("name: X\nac: 10\nhp: 1\nspeed: 30", "unknown field `speed`"),
    ];
    for (yaml, want) in cases {
        let err = StatBlock::from_yaml(yaml).unwrap_err();
        assert!(err.contains(want), "expected `{want}` in `{err}`");
    }
}
