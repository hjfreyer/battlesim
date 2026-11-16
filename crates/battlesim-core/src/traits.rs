//! Traits loaded from data, behind the same interface as the Rust rules.

use crate::data::{Node, TraitDef};
use crate::event::Event;
use crate::expr::Env;
use crate::rule::{Before, Entry, Proposal, Rule, Scope, Step, Trigger, View};
use crate::state::Id;

/// One trait, on one creature. It comes from the creature's stat block or
/// from an effect attached to it.
pub struct TraitRule<'a> {
    pub owner: Id,
    pub def: &'a TraitDef,
    /// For a trait carried by an effect, whoever applied the effect.
    pub source: Option<Id>,
}

impl TraitRule<'_> {
    /// Whether the trait applies to this event as things stand now. This is
    /// asked again each time the rule's turn could come, never cached.
    pub fn matches(&self, event: &Event, view: &View) -> bool {
        let (def, state) = (self.def, view.state);
        if def.ignored || !state.in_fight(self.owner) || !event.involves(def.role, self.owner) {
            return false;
        }
        if def.optional && !state.can_act(self.owner) {
            return false;
        }
        // A rule that cannot pay is skipped rather than proposed and left to
        // fizzle. Fizzling would give the same result with a noisier log.
        if !state.can_pay(self.owner, &def.uses) {
            return false;
        }
        let block = &state.creature(self.owner).block;
        for node in &def.effect {
            if let Node::Use(u) = node
                && u.as_action
                && !block.action(&u.action).is_some_and(|a| state.affordable(self.owner, a))
            {
                return false;
            }
        }
        // An effect aimed at something already out of the fight would only
        // spend its cost for nothing.
        if !def.effect.is_empty() && event.target().is_some_and(|t| !state.in_fight(t)) {
            return false;
        }
        let env = Env { state, me: self.owner, target: None, event: Some(event), action: None, source: self.source };
        def.when.as_ref().is_none_or(|when| when.test(&env))
    }

    fn scope(&self, event: &Event, view: &View) -> Scope {
        let inherit = match event {
            Event::Damage { kind, .. } => Some(kind.clone()),
            _ => event.action_ref().and_then(|(owner, name)| {
                view.state.creature(owner).block.action(name)?.damage_type().map(str::to_string)
            }),
        };
        Scope {
            actor: self.owner,
            target: event.target().or(Some(self.owner)),
            action: None,
            trigger: Some(Trigger { event: event.clone(), pending: view.pending }),
            crit: matches!(event, Event::Hit(o) if o.crit),
            inherit,
        }
    }

    /// The trait as an effect list: what it costs, then what it does.
    fn body(&self, before: bool) -> Vec<Step> {
        let def = self.def;
        let mut steps: Vec<Step> = def
            .uses
            .iter()
            .map(|(resource, n)| Step::Spend { who: self.owner, resource: resource.clone(), n: *n })
            .collect();
        if before {
            steps.extend(def.change.clone().map(Step::Change));
            if def.cancel {
                steps.push(Step::Cancel);
            }
        }
        steps.extend(def.effect.iter().cloned().map(Step::Node));
        steps
    }

    fn propose(&self, steps: Vec<Step>, scope: Scope) -> Vec<Proposal> {
        if self.def.optional {
            vec![Proposal::Ask { actor: self.owner, rule: self.def.label().to_string(), steps, scope }]
        } else if steps.is_empty() {
            Vec::new()
        } else {
            vec![Proposal::Steps(steps, scope)]
        }
    }
}

impl Rule for TraitRule<'_> {
    fn before(&self, event: &Event, view: &View) -> Before {
        let def = self.def;
        if def.has_body() {
            return Before::Propose(self.propose(self.body(true), self.scope(event, view)));
        }
        // With nothing to pay, run or ask, the rule is a plain verdict.
        if def.cancel {
            return Before::Cancel;
        }
        match def.change.as_ref().map(|change| change.apply(event)) {
            Some(next) if next != *event => Before::Change(next),
            _ => Before::Pass,
        }
    }

    fn after(&self, entry: &Entry, view: &View) -> Vec<Proposal> {
        self.propose(self.body(false), self.scope(&entry.event, view))
    }
}
