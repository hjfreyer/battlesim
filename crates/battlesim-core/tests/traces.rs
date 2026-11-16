//! The three worked traces from the engine design doc, as tests.

mod common;
use battlesim_core::{Combatant, Expect, Setup};
use common::*;

#[test]
fn sneak_attack() {
    let gnoll = block(GNOLL);
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &block(ROGUE)),
            Combatant::new("Gnoll", "monsters", &gnoll).condition("prone"),
            Combatant::new("Other gnoll", "monsters", &gnoll),
        ],
        engaged: vec![(0, 1)],
    };
    let mut fight = Fight::start(setup, &[20, 5, 3]);
    let mark = fight.mark() - 1;
    fight.act("Alice", "Rapier", Some("Gnoll"));
    fight.roll("2d20 keep highest", 17).roll("1d8+4", 9).roll("3d6", 11);

    let expected = "\
#5 TurnStarted(Alice)
#6   Chose(Alice, step, Rapier at Gnoll)
#7   ResourceSpent(Alice, action, 1)
#8   ActionUsed(Alice, Rapier, [Gnoll])
#9     Attack(Alice, Gnoll, Rapier, +7, advantage)
#10       Rolled(2d20 keep highest, 17)
#11       Hit(#9, total 24, AC 15)
#12         Rolled(1d8+4, 9)
#13         Damage(Gnoll, 9 piercing)
#14         ResourceSpent(Alice, sneak_attack, 1)
#15         Rolled(3d6, 11)
#16         Damage(Gnoll, 11 piercing)
#17           Died(Gnoll)
#18   Chose(Alice, step, end turn)
#19 TurnEnded(Alice)
#20 TurnStarted(Other gnoll)
";
    assert_eq!(fight.trace_from(mark), expected);
    // Once per turn means any turn: it is back as soon as the next one starts.
    assert_eq!(fight.pool("Alice", "sneak_attack"), 1);
}

#[test]
fn sneak_attack_is_not_spent_when_the_weapon_already_killed() {
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &block(ROGUE)),
            Combatant::new("Gnoll", "monsters", &block(GNOLL)).condition("prone").hp(5),
            Combatant::new("Other gnoll", "monsters", &block(GNOLL)),
        ],
        engaged: vec![(0, 1)],
    };
    let mut fight = Fight::start(setup, &[20, 5, 3]);
    let mark = fight.mark();
    fight.act("Alice", "Rapier", Some("Gnoll")).roll("2d20 keep highest", 17).roll("1d8+4", 9);
    let trace = fight.trace_from(mark);
    assert!(trace.contains("Died(Gnoll)"), "{trace}");
    assert!(!trace.contains("sneak_attack"), "{trace}");
}

#[test]
fn sneak_attack_needs_more_than_an_ally_when_the_roll_has_disadvantage() {
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &block(ROGUE)).condition("prone"),
            Combatant::new("Bob", "party", &block(FIGHTER)),
            Combatant::new("Gnoll", "monsters", &block(GNOLL)),
        ],
        engaged: vec![(1, 2)],
    };
    let mut fight = Fight::start(setup, &[20, 10, 1]);
    fight.act("Alice", "Rapier", Some("Gnoll")).roll("2d20 keep lowest", 15).roll("1d8+4", 6);
    // Bob is on the gnoll, but Alice attacked from the ground.
    assert_eq!(fight.pool("Alice", "sneak_attack"), 1);
    fight.act("Bob", "Longsword", Some("Gnoll"));
}

#[test]
fn shield_turns_a_hit_into_a_miss() {
    let mut fight = Fight::duel(("Wizard", WIZARD), ("Ogre", OGRE), &[1, 20]);
    let mark = fight.mark() - 1;
    fight.act("Ogre", "Greatclub", Some("Wizard")).roll("1d20", 11);
    fight.react("Wizard", "Shield", true);

    let expected = "\
#4 TurnStarted(Ogre)
#5   Chose(Ogre, step, Greatclub at Wizard)
#6   ResourceSpent(Ogre, action, 1)
#7   ActionUsed(Ogre, Greatclub, [Wizard])
#8     Attack(Ogre, Wizard, Greatclub, +6)
#9       Engaged(Ogre, Wizard)
#10       Rolled(1d20, 11)
#11       Chose(Wizard, react, Shield)
#12       ResourceSpent(Wizard, reaction, 1)
#13       ResourceSpent(Wizard, spell_slot_1, 1)
#14       EffectAttached(Wizard, Shield)
#15       Miss(#8, total 17, AC 20)
#16   Chose(Ogre, step, end turn)
#17 TurnEnded(Ogre)
#18 TurnStarted(Wizard)
#19   EffectRemoved(Wizard, Shield)
";
    assert_eq!(fight.trace_from(mark), expected);
    assert_eq!(fight.creature("Wizard").hp, 27);
    assert_eq!(fight.pool("Wizard", "spell_slot_1"), 3);
    // Her turn started, so the reaction is back and Shield is gone.
    assert_eq!(fight.pool("Wizard", "reaction"), 1);
    assert!(fight.creature("Wizard").effects.is_empty());
}

#[test]
fn declining_shield_lets_the_hit_land() {
    let mut fight = Fight::duel(("Wizard", WIZARD), ("Ogre", OGRE), &[1, 20]);
    fight.act("Ogre", "Greatclub", Some("Wizard")).roll("1d20", 11);
    fight.react("Wizard", "Shield", false).roll("2d8+4", 13);
    assert!(fight.has_line("Chose(Wizard, react, pass on Shield)"));
    assert!(fight.has_line("Hit(#8, total 17, AC 15)"));
    assert_eq!(fight.creature("Wizard").hp, 14);
    assert_eq!(fight.pool("Wizard", "spell_slot_1"), 4);
    assert_eq!(fight.pool("Wizard", "reaction"), 1);
}

#[test]
fn shield_is_not_offered_without_a_slot_or_a_reaction() {
    for pool in ["spell_slot_1", "reaction"] {
        let setup = Setup {
            combatants: vec![
                Combatant::new("Wizard", "party", &block(WIZARD)).resource(pool, 0),
                Combatant::new("Ogre", "monsters", &block(OGRE)),
            ],
            ..Setup::default()
        };
        let mut fight = Fight::start(setup, &[1, 20]);
        fight.act("Ogre", "Greatclub", Some("Wizard")).roll("1d20", 11);
        // Straight to the damage roll, with no prompt in between.
        assert!(matches!(fight.engine.expects(), Expect::Roll { .. }), "no prompt expected without {pool}");
        assert!(fight.has_line("Hit(#8, total 17, AC 15)"));
    }
}

#[test]
fn a_countered_counterspell() {
    let abjurer = block(ABJURER);
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &block(EVOKER)),
            Combatant::new("Bob", "party", &abjurer),
            Combatant::new("Mage", "monsters", &abjurer),
        ],
        ..Setup::default()
    };
    let mut fight = Fight::start(setup, &[20, 10, 5]);
    let mark = fight.mark() - 1;
    fight.act("Alice", "Fireball", Some("Mage"));
    fight.react("Mage", "Counterspell", true);
    fight.react("Bob", "Counterspell", true);
    fight.roll("1d20", 9); // the Mage's Constitution save against Bob's Counterspell
    fight.roll("1d20", 14).roll("8d6", 28); // the Mage's Dexterity save against Fireball, then the damage

    let expected = "\
#5 TurnStarted(Alice)
#6   Chose(Alice, step, Fireball at Mage)
#7   ResourceSpent(Alice, action, 1)
#8   Chose(Mage, react, Counterspell)
#9   ResourceSpent(Mage, reaction, 1)
#10   Chose(Bob, react, Counterspell)
#11   ResourceSpent(Bob, reaction, 1)
#12   ActionUsed(Bob, Counterspell, [])
#13     ResourceSpent(Bob, spell_slot_3, 1)
#14     Save(Mage, con, DC 15)
#15       Rolled(1d20, 9)
#16       SaveFailed(#14, total 9, DC 15)
#17   Fizzled(ActionUsed(Mage, Counterspell, []), cancelled by #16)
#18   ActionUsed(Alice, Fireball, [Mage])
#19     ResourceSpent(Alice, spell_slot_3, 1)
#20     Save(Mage, dex, DC 15)
#21       Rolled(1d20, 14)
#22       SaveFailed(#20, total 14, DC 15)
#23         Rolled(8d6, 28)
#24         Damage(Mage, 28 fire)
#25   Chose(Alice, step, end turn)
#26 TurnEnded(Alice)
#27 TurnStarted(Bob)
";
    assert_eq!(fight.trace_from(mark), expected);
    // The 2024 wording: a countered spell wastes the action used to cast it
    // but not the slot.
    assert_eq!((fight.pool("Mage", "reaction"), fight.pool("Mage", "spell_slot_3")), (0, 2));
    assert_eq!((fight.pool("Bob", "reaction"), fight.pool("Bob", "spell_slot_3")), (1, 1));
    assert_eq!(fight.pool("Alice", "spell_slot_3"), 1);
    assert_eq!(fight.creature("Mage").hp, 12);
}

#[test]
fn a_counterspell_that_is_saved_against_does_nothing() {
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &block(EVOKER)),
            Combatant::new("Mage", "monsters", &block(ABJURER)),
        ],
        ..Setup::default()
    };
    let mut fight = Fight::start(setup, &[20, 5]);
    fight.act("Alice", "Fireball", Some("Mage"));
    fight.react("Mage", "Counterspell", true);
    fight.roll("1d20", 15); // Alice's Constitution save succeeds
    fight.roll("1d20", 3).roll("8d6", 30);
    assert!(fight.has_line("ActionUsed(Mage, Counterspell, [])"));
    assert!(fight.has_line("ActionUsed(Alice, Fireball, [Mage])"));
    assert_eq!(fight.pool("Mage", "spell_slot_3"), 1);
    assert_eq!(fight.creature("Mage").hp, 10);
}

#[test]
fn a_successful_counterspell_wastes_the_action_but_not_the_slot() {
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &block(EVOKER)),
            Combatant::new("Mage", "monsters", &block(ABJURER)),
        ],
        ..Setup::default()
    };
    let mut fight = Fight::start(setup, &[20, 5]);
    fight.act("Alice", "Fireball", Some("Mage"));
    fight.react("Mage", "Counterspell", true);
    fight.roll("1d20", 4); // Alice fails
    assert!(fight.lines().iter().any(|l| l.starts_with("Fizzled(ActionUsed(Alice, Fireball, [Mage]), cancelled by")));
    assert_eq!((fight.pool("Alice", "action"), fight.pool("Alice", "spell_slot_3")), (0, 2));
    assert_eq!(fight.creature("Mage").hp, 40);
}
