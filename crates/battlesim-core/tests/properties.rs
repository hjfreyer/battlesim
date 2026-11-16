//! Properties of the engine itself: refusing bad input, fizzling, loop
//! limits, and replay.

mod common;
use battlesim_core::drive::{self, DiceSource, FirstOption, Player, RandomPlayer, SeededDice};
use battlesim_core::{
    Before, Choice, Combatant, Config, Engine, Entry, Event, Expect, Fault, Id, Input, Invalid, Line, Proposal, Rule,
    Scope, Setup, Step, View, trace,
};
use common::*;

#[test]
fn a_refused_input_changes_nothing() {
    let mut fight = Fight::duel(("Fighter", FIGHTER), ("Gnoll", GNOLL), &[20, 1]);
    let snapshot = |f: &Fight| (f.engine.state().clone(), f.engine.log().clone(), f.engine.expects().clone());

    let before = snapshot(&fight);
    let gnoll = fight.id("Gnoll");
    let fighter = fight.id("Fighter");
    let act = |action: &str, target: Id| Input::Chose(Choice::Act { action: action.into(), targets: vec![target] });
    let refused = [
        (Input::Rolled(10), Invalid::ExpectedChoice),
        (act("Fireball", gnoll), Invalid::NotAllowed), // not an action it has
        (act("Longsword", fighter), Invalid::NotAllowed), // not a legal target
        (act("Second Wind", gnoll), Invalid::NotAllowed), // aimed at the wrong creature
        (Input::Chose(Choice::React(true)), Invalid::NotAllowed), // not what was asked
        (Input::Chose(Choice::Target(gnoll)), Invalid::NotAllowed),
    ];
    for (input, why) in refused {
        assert_eq!(fight.engine.push(input), Err(why));
        assert!(snapshot(&fight) == before);
    }

    fight.act("Fighter", "Longsword", Some("Gnoll"));
    let before = snapshot(&fight);
    let refused = [
        (Input::Rolled(0), Invalid::OutOfRange { min: 1, max: 20 }),
        (Input::Rolled(21), Invalid::OutOfRange { min: 1, max: 20 }),
        (Input::Chose(Choice::EndTurn), Invalid::ExpectedRoll),
    ];
    for (input, why) in refused {
        assert_eq!(fight.engine.push(input), Err(why));
        assert!(snapshot(&fight) == before);
    }
    fight.roll("1d20", 15);
    assert_eq!(fight.engine.push(Input::Rolled(12)), Err(Invalid::OutOfRange { min: 4, max: 11 }));
}

#[test]
fn an_action_that_cannot_be_paid_for_is_not_an_option() {
    let mut fight = Fight::duel(("Fighter", FIGHTER), ("Gnoll", GNOLL), &[20, 1]);
    let wind = Choice::Act { action: "Second Wind".into(), targets: vec![fight.id("Fighter")] };
    assert!(fight.engine.options().contains(&wind));
    fight.act("Fighter", "Second Wind", Some("Fighter")).roll("1d10+5", 9);
    // The bonus action and the use are both gone.
    assert!(!fight.engine.options().contains(&wind));
    assert_eq!(fight.engine.push(Input::Chose(wind)), Err(Invalid::NotAllowed));
}

/// Proposes the same paid effect twice when round one starts. Proposing is
/// unrestricted; only one of the two can take effect.
struct PayTwice;

impl Rule for PayTwice {
    fn after(&self, entry: &Entry, _: &View) -> Vec<Proposal> {
        if entry.event != (Event::RoundStarted { round: 1 }) {
            return Vec::new();
        }
        let me = Id(0);
        let paid = || {
            let steps = vec![
                Step::Spend { who: me, resource: "reaction".into(), n: 1 },
                Step::Event(Event::TempHp { target: me, amount: 5 }),
            ];
            Proposal::Steps(steps, Scope::of(me))
        };
        vec![paid(), paid()]
    }
}

#[test]
fn spending_the_same_resource_twice_fizzles_the_second_and_what_it_paid_for() {
    let setup = Setup {
        combatants: vec![
            Combatant::new("Fighter", "party", &block(FIGHTER)),
            Combatant::new("Gnoll", "monsters", &block(GNOLL)),
        ],
        ..Setup::default()
    };
    let mut engine = Engine::with_rules(&setup, Config::default(), vec![Box::new(PayTwice)]).unwrap();
    // The gnoll goes first, so the fighter's reaction is not refilled yet.
    drive::replay(&mut engine, [Input::Rolled(1), Input::Rolled(20)]).unwrap();
    let lines: Vec<String> = engine.log().events().map(|e| trace::describe(e, engine.state())).collect();
    let count = |prefix: &str| lines.iter().filter(|l| l.starts_with(prefix)).count();
    assert_eq!(count("ResourceSpent(Fighter, reaction, 1)"), 1);
    assert_eq!(count("TempHp(Fighter, 5)"), 1);
    assert_eq!(count("Fizzled(ResourceSpent(Fighter, reaction, 1), the resource is already spent)"), 1);
    // A fizzle is a marker, not an undo: the first payment stands.
    assert_eq!(engine.state().creature(Id(0)).temp_hp, 5);
    assert_eq!(engine.state().creature(Id(0)).resource("reaction"), 0);
}

/// Answers every heal with another heal, forever.
struct Echo;

impl Rule for Echo {
    fn after(&self, entry: &Entry, _: &View) -> Vec<Proposal> {
        match entry.event {
            Event::RoundStarted { .. } | Event::Heal { .. } => {
                vec![Proposal::Event(Event::Heal { target: Id(0), amount: 1 })]
            }
            _ => Vec::new(),
        }
    }
}

/// Makes every pending heal wait for another heal, forever.
struct Nest;

impl Rule for Nest {
    fn before(&self, event: &Event, _: &View) -> Before {
        match event {
            Event::Heal { .. } => Before::Propose(vec![Proposal::Event(event.clone())]),
            _ => Before::Pass,
        }
    }

    fn after(&self, entry: &Entry, _: &View) -> Vec<Proposal> {
        match entry.event {
            Event::RoundStarted { .. } => vec![Proposal::Event(Event::Heal { target: Id(0), amount: 1 })],
            _ => Vec::new(),
        }
    }
}

fn looping(rule: Box<dyn Rule>) -> Engine {
    let setup = Setup {
        combatants: vec![
            Combatant::new("Fighter", "party", &block(FIGHTER)),
            Combatant::new("Gnoll", "monsters", &block(GNOLL)),
        ],
        ..Setup::default()
    };
    let mut engine = Engine::with_rules(&setup, Config::default(), vec![rule]).unwrap();
    engine.push(Input::Rolled(10)).unwrap();
    engine.push(Input::Rolled(10)).unwrap();
    engine
}

#[test]
fn rules_that_loop_end_the_fight_as_an_error() {
    // After-rules that feed each other never deepen the stack, because a
    // finished event leaves before its consequences run. The event count
    // catches them.
    let engine = looping(Box::new(Echo));
    assert_eq!(engine.fault(), Some(Fault::TooManyEvents));
    assert_eq!(engine.expects(), &Expect::Done);
    assert!(!engine.state().ended);

    // Before-rules that feed each other do deepen it.
    let mut engine = looping(Box::new(Nest));
    assert_eq!(engine.fault(), Some(Fault::StackTooDeep));
    assert_eq!(engine.push(Input::Rolled(1)), Err(Invalid::Faulted(Fault::StackTooDeep)));
}

#[test]
fn a_fight_nobody_can_win_is_a_draw_at_the_round_limit() {
    let statue = block("name: Statue\nac: 17\nhp: 50\n");
    let setup = Setup {
        combatants: vec![Combatant::new("Left", "a", &statue), Combatant::new("Right", "b", &statue)],
        ..Setup::default()
    };
    let config = Config { max_rounds: 3, ..Config::default() };
    let mut engine = Engine::with_rules(&setup, config, Vec::new()).unwrap();
    // Neither has anything to do, so every turn ends without asking and
    // the two initiative rolls are the only inputs the whole fight needs.
    drive::replay(&mut engine, [Input::Rolled(10), Input::Rolled(10)]).unwrap();
    assert_eq!(engine.expects(), &Expect::Done);
    assert_eq!(engine.fault(), None);
    assert!(engine.state().ended);
    assert_eq!(engine.state().winner, None);
    assert_eq!(engine.state().round, 4);
    assert_eq!(engine.log().inputs().len(), 2);
}

fn skirmish() -> Setup {
    let wolf = block(WOLF);
    Setup {
        combatants: vec![
            Combatant::new("Fighter", "party", &block(FIGHTER)),
            Combatant::new("Rogue", "party", &block(ROGUE)),
            Combatant::new("Wizard", "party", &block(WIZARD)).back(),
            Combatant::new("Ogre", "monsters", &block(OGRE)),
            Combatant::new("Grey", "monsters", &wolf),
            Combatant::new("Red", "monsters", &wolf),
            Combatant::new("Mage", "monsters", &block(WIZARD)).back(),
        ],
        ..Setup::default()
    }
}

#[test]
fn a_fight_replays_exactly_from_its_inputs() {
    for seed in 0..40 {
        let mut engine = Engine::new(&skirmish()).unwrap();
        drive::run(&mut engine, &mut RandomPlayer(SeededDice::new(seed)), &mut SeededDice::new(seed + 1000)).unwrap();
        assert_eq!(engine.fault(), None, "seed {seed}");
        assert!(engine.state().ended, "seed {seed}");

        let mut again = Engine::new(&skirmish()).unwrap();
        drive::replay(&mut again, engine.log().inputs()).unwrap();
        assert!(again.log() == engine.log(), "seed {seed}: the replayed log differs");
        assert!(again.state() == engine.state(), "seed {seed}: the replayed state differs");
        assert_eq!(again.expects(), &Expect::Done);
    }
}

#[test]
fn random_fights_end_cleanly_and_keep_the_state_sane() {
    let mut winners = std::collections::BTreeMap::new();
    let (mut longest, mut deepest) = (0, 0);
    for seed in 0..300 {
        let mut engine = Engine::new(&skirmish()).unwrap();
        let mut dice = SeededDice::new(seed);
        let mut random = RandomPlayer(SeededDice::new(seed * 7 + 1));
        let player: &mut dyn Player = if seed % 2 == 0 { &mut FirstOption } else { &mut random };
        // The same loop as `drive::run`, watching the stack as it goes.
        loop {
            deepest = deepest.max(engine.stack_depth());
            let input = match engine.expects() {
                Expect::Done => break,
                Expect::Roll { spec, .. } => Input::Rolled(dice.roll(spec)),
                Expect::Choice { actor, constraint } => {
                    Input::Chose(player.choose(&engine, *actor, constraint, &engine.options()))
                }
            };
            engine.push(input).unwrap();
        }
        let state = engine.state();
        assert_eq!(engine.fault(), None, "seed {seed}");
        assert!(state.ended, "seed {seed}");
        for c in &state.creatures {
            assert!(c.hp <= c.block.hp, "seed {seed}: {} has more than full hit points", c.name);
            for (name, amount) in &c.resources {
                assert!(*amount <= state.max_resource(c.id, name), "seed {seed}: {} overflowed {name}", c.name);
            }
            assert!(c.line == Line::Front || c.name == "Wizard" || c.name == "Mage");
        }
        let standing = state.standing_sides();
        assert!(standing.len() <= 1, "seed {seed}: the fight ended with both sides standing");
        assert_eq!(state.winner.as_deref(), standing.first().copied(), "seed {seed}");
        *winners.entry(state.winner.clone()).or_insert(0) += 1;
        longest = longest.max(engine.log().len());
    }
    // Both sides can win this one, which says the fight is a real contest
    // and not a script that always goes one way.
    assert!(winners.get(&Some("party".to_string())).is_some_and(|n| *n > 10), "{winners:?}");
    assert!(winners.get(&Some("monsters".to_string())).is_some_and(|n| *n > 10), "{winners:?}");
    assert!(longest < 5_000, "a fight ran to {longest} events");
    // The stack does not grow with the length of the fight.
    assert!(deepest <= 12, "the stack reached {deepest} items");
}
