# battlesim

A simulator for Dungeons & Dragons fights (2024 rules), built to answer one
question for a DM or an agent working for one: how much does this encounter
tax the party, and how likely is it to kill them?

This repository currently holds the engine. The command-line interface,
policies and reports come next.

## The engine in one page

The engine is built from five ideas.

- **State.** Who is in the fight, their hit points, resources, conditions and
  positions.
- **Events.** Everything that happens is an event: `Attack`, `Hit`, `Damage`,
  `TurnStarted`, and so on. Only a committed event changes state.
- **A stack.** Proposed events, effect lists and requests for input wait on an
  explicit stack. The engine works on whatever is on top.
- **Rules.** A rule responds to an event, before it commits or after. Before,
  it can change, cancel or replace the event, or propose things that must
  resolve first. After, it can propose more. Core D&D rules are Rust; traits
  from stat blocks are data. Both sit behind the same interface.
- **Inputs.** The engine is passive. It has no policy and no dice. It says what
  it needs with `expects()`, a choice or a roll, and takes it with `push()`.
  Anything can supply inputs: a policy, a seeded roller, a recorded log, a
  test.

An event that can no longer happen when its turn comes, because its target
died or its cost was already spent, fizzles: it is dropped, a marker is
logged, and nothing is undone.

```rust
use battlesim_core::{Engine, Expect, Input};

let mut engine = Engine::new(&setup)?;
loop {
    let input = match engine.expects() {
        Expect::Done => break,
        Expect::Roll { spec, .. } => Input::Rolled(dice.roll(spec)),
        Expect::Choice { actor, constraint } => Input::Chose(policy.choose(&engine, *actor, constraint, &engine.options())),
    };
    engine.push(input)?;
}
```

The log is the result. `log.inputs()` gives back the inputs that produced it,
and pushing those into a fresh engine reproduces the fight exactly.

## Stat blocks

A player character and a monster are the same type, loaded from YAML or JSON.
Abilities are declarative: a small vocabulary of effect nodes and a small
expression language, not scripts.

```yaml
name: Wolf
ac: 12
hp: 11
abilities: {dex: 15}
actions:
  - name: Bite
    target: melee
    effect:
      - attack:
          bonus: 4
          on_hit:
            - damage: {dice: 1d6+2, type: piercing}
traits:
  - name: Pack Tactics
    before: Attack
    as: attacker
    when: "ally_engaged_with(target)"
    change: {advantage: true}
```

Names are checked when a stat block loads. A misspelled function, field,
resource or condition is an error that lists the valid ones.

## Layout

- `crates/battlesim-core/src/engine.rs`: the stack, the event lifecycle,
  fizzling, `expects` and `push`.
- `core_rules.rs`: turns, attacks, saves, damage, dying, and each event's
  requirement.
- `rule.rs`, `traits.rs`: the rule interface and traits-as-rules.
- `data.rs`, `expr.rs`, `dice.rs`: the stat block schema, the expression
  language, dice.
- `data/conditions.yaml`: the built-in conditions, written as traits.
- `drive.rs`, `trace.rs`: simple proposers and a text rendering of the log.

## Running it

```sh
cargo test
cargo run --example skirmish -- 7   # one fight with seed 7, printed as a trace
```

## Not here yet

Movement and opportunity attacks, area effects, concentration, effects that
end on a repeated save, bonuses that are themselves dice, legendary actions,
the command-line interface, policies and the encounter report.
