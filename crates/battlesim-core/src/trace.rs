//! Renders a log as text, one event per line, indented by cause.

use crate::event::{Choice, Constraint, Event, Fizzle};
use crate::rule::{Entry, Log};
use crate::state::{Id, State};

/// The whole log. Structure events sit at the left margin; everything else
/// is indented one step further than the event that caused it.
pub fn render(log: &Log, state: &State) -> String {
    render_entries(log.entries(), log, state)
}

/// Part of the log, indented as it would be in the whole.
pub fn render_entries(entries: &[Entry], log: &Log, state: &State) -> String {
    let mut depths: Vec<usize> = Vec::with_capacity(log.len());
    for entry in log.entries() {
        let depth = match entry.cause {
            _ if entry.event.is_structure() => 0,
            Some(cause) => depths[cause.0 as usize] + 1,
            None => 0,
        };
        depths.push(depth);
    }
    let mut out = String::new();
    for entry in entries {
        let indent = "  ".repeat(depths[entry.id.0 as usize]);
        out.push_str(&format!("#{} {indent}{}\n", entry.id.0, describe(&entry.event, state)));
    }
    out
}

fn names(ids: &[Id], state: &State) -> String {
    ids.iter().map(|id| state.name(*id)).collect::<Vec<_>>().join(", ")
}

/// One event as a short line of text.
pub fn describe(event: &Event, state: &State) -> String {
    let name = |id: &Id| state.name(*id);
    match event {
        Event::Damage { target, kind, resist, vulnerable, immune, .. } => {
            let note = match (immune, resist, vulnerable) {
                (true, _, _) => ", immune",
                (_, true, true) => ", resisted and vulnerable",
                (_, true, _) => ", resisted",
                (_, _, true) => ", vulnerable",
                _ => "",
            };
            format!("Damage({}, {} {kind}{note})", name(target), event.damage_dealt())
        }
        Event::Heal { target, amount } => format!("Heal({}, {amount})", name(target)),
        Event::TempHp { target, amount } => format!("TempHp({}, {amount})", name(target)),
        Event::ResourceSpent { who, resource, n } => format!("ResourceSpent({}, {resource}, {n})", name(who)),
        Event::ResourceRestored { who, resource, n } => format!("ResourceRestored({}, {resource}, {n})", name(who)),
        Event::EffectAttached { target, effect, .. } => format!("EffectAttached({}, {})", name(target), effect.label()),
        Event::EffectRemoved { target, name: effect, .. } => format!("EffectRemoved({}, {effect})", name(target)),
        Event::Moved { who, to } => format!("Moved({}, {to:?})", name(who)),
        Event::Engaged { a, b } => format!("Engaged({}, {})", name(a), name(b)),
        Event::Disengaged { a, b } => format!("Disengaged({}, {})", name(a), name(b)),
        Event::Dropped { who } => format!("Dropped({})", name(who)),
        Event::Died { who } => format!("Died({})", name(who)),
        Event::ActionUsed { actor, action, targets } => {
            format!("ActionUsed({}, {action}, [{}])", name(actor), names(targets, state))
        }
        Event::Attack { attacker, target, action, bonus, advantage, disadvantage, .. } => {
            let mode = match (advantage, disadvantage) {
                (true, false) => ", advantage",
                (false, true) => ", disadvantage",
                _ => "",
            };
            format!("Attack({}, {}, {action}, {bonus:+}{mode})", name(attacker), name(target))
        }
        Event::Hit(o) => {
            format!("Hit(#{}, total {}, AC {}{})", o.attack.0, o.total, o.ac, if o.crit { ", critical" } else { "" })
        }
        Event::Miss(o) => format!("Miss(#{}, total {}, AC {})", o.attack.0, o.total, o.ac),
        Event::Save { who, ability, dc, .. } => format!("Save({}, {}, DC {dc})", name(who), ability.name()),
        Event::SaveFailed { save, total, dc, .. } => format!("SaveFailed(#{}, total {total}, DC {dc})", save.0),
        Event::SaveSucceeded { save, total, dc, .. } => format!("SaveSucceeded(#{}, total {total}, DC {dc})", save.0),
        Event::DeathSave { who } => format!("DeathSave({})", name(who)),
        Event::FightStarted => "FightStarted".to_string(),
        Event::RoundStarted { round } => format!("RoundStarted({round})"),
        Event::TurnStarted { who } => format!("TurnStarted({})", name(who)),
        Event::TurnEnded { who } => format!("TurnEnded({})", name(who)),
        Event::FightEnded { winner } => format!("FightEnded({})", winner.as_deref().unwrap_or("draw")),
        Event::Fizzled { event, reason } => {
            let why = match reason {
                Fizzle::Requirement(why) => (*why).to_string(),
                Fizzle::Cancelled(Some(by)) => format!("cancelled by #{}", by.0),
                Fizzle::Cancelled(None) => "cancelled by a rule".to_string(),
            };
            format!("Fizzled({}, {why})", describe(event, state))
        }
        Event::Rolled { spec, value, .. } => format!("Rolled({spec}, {value})"),
        Event::Chose { actor, prompt, choice, .. } => {
            let what = match (prompt, choice) {
                (_, Choice::Act { action, targets }) if targets.is_empty() => format!("step, {action}"),
                (_, Choice::Act { action, targets }) => format!("step, {action} at {}", names(targets, state)),
                (_, Choice::EndTurn) => "step, end turn".to_string(),
                (Constraint::React { rule, .. }, Choice::React(true)) => format!("react, {rule}"),
                (Constraint::React { rule, .. }, Choice::React(false)) => format!("react, pass on {rule}"),
                (Constraint::Target { action }, Choice::Target(t)) => format!("target, {action} at {}", name(t)),
                (_, other) => format!("{other:?}"),
            };
            format!("Chose({}, {what})", name(actor))
        }
        Event::Ac { who, value } => format!("Ac({}, {value})", name(who)),
    }
}
