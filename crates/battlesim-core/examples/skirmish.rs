//! Runs one small fight with seeded dice and prints its trace.
//!
//! ```text
//! cargo run --example skirmish -- 7
//! ```

use battlesim_core::drive::{self, FirstOption, SeededDice};
use battlesim_core::{Combatant, Engine, Setup, StatBlock, trace};
use std::sync::Arc;

const ROGUE: &str = r#"
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

const FIGHTER: &str = r#"
name: Fighter 5
ac: 18
hp: 44
death_saves: true
actions:
  - name: Longsword
    target: melee
    effect:
      - attack: {bonus: 6, on_hit: [{damage: {dice: 1d8+3, type: slashing}}]}
"#;

const WOLF: &str = r#"
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seed = std::env::args().nth(1).map(|s| s.parse()).transpose()?.unwrap_or(1);
    let load = |yaml: &str| StatBlock::from_yaml(yaml).map(Arc::new);
    let wolf = load(WOLF)?;
    let setup = Setup {
        combatants: vec![
            Combatant::new("Alice", "party", &load(ROGUE)?),
            Combatant::new("Bob", "party", &load(FIGHTER)?),
            Combatant::new("Grey", "wolves", &wolf),
            Combatant::new("Red", "wolves", &wolf),
            Combatant::new("Ash", "wolves", &wolf),
        ],
        ..Setup::default()
    };
    let mut engine = Engine::new(&setup)?;
    drive::run(&mut engine, &mut FirstOption, &mut SeededDice::new(seed))?;
    print!("{}", trace::render(engine.log(), engine.state()));
    Ok(())
}
