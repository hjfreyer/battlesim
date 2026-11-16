//! Rules, and what they can ask the engine to do.
//!
//! A rule is a pure function of an event and a read-only view. Before an
//! event commits, a rule can change it, cancel it, replace it, or propose
//! things that must resolve first. After it commits, a rule can propose more.
//! A rule never waits for an answer: what it proposes goes on the stack, and
//! whatever follows from that is some other rule's business.

use crate::data::{Band, Change, EventKind, Node};
use crate::engine::{Config, Item};
use crate::event::{Event, EventId, Expect, Input};
use crate::expr::Env;
use crate::state::{Id, State};
use crate::traits::TraitRule;
use std::ops::Index;
use std::sync::Arc;

/// A proposed event that has not committed yet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ProposalId(pub u32);

/// The context an effect runs in: who is acting, on whom, as part of what.
/// It is recorded with every event an effect proposes, the way the cause is,
/// so a later rule can pick up where the effect left off.
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub actor: Id,
    pub target: Option<Id>,
    /// The action being carried out, by name. It belongs to `actor`.
    pub action: Option<String>,
    /// The event this effect is a response to.
    pub trigger: Option<Trigger>,
    /// Damage dice are doubled.
    pub crit: bool,
    /// The damage type that `type: inherit` resolves to.
    pub inherit: Option<String>,
}

impl Scope {
    pub fn of(actor: Id) -> Scope {
        Scope { actor, target: None, action: None, trigger: None, crit: false, inherit: None }
    }

    pub fn env<'a>(&'a self, state: &'a State) -> Env<'a> {
        Env {
            state,
            me: self.actor,
            target: self.target,
            event: self.trigger.as_ref().map(|t| &t.event),
            action: self.action.as_deref().map(|name| (self.actor, name)),
            source: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub event: Event,
    /// Set while the event is still pending, which is when it can be changed
    /// or cancelled.
    pub pending: Option<ProposalId>,
}

/// One entry of an effect list. The list runs in order, each entry resolving
/// fully before the next starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// A cost. If it fizzles, the rest of the list is abandoned.
    Spend {
        who: Id,
        resource: String,
        n: u32,
    },
    Event(Event),
    Node(Node),
    /// A nested list with its own scope.
    Scoped(Vec<Step>, Scope),
    /// Edit the pending event this list is a response to.
    Change(Change),
    /// Cancel the pending event this list is a response to.
    Cancel,
}

/// Something a rule wants to happen.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposal {
    Event(Event),
    Steps(Vec<Step>, Scope),
    /// Ask for an input.
    Need(Expect),
    /// Ask `actor` whether to run the steps, and run them on a yes.
    Ask {
        actor: Id,
        rule: String,
        steps: Vec<Step>,
        scope: Scope,
    },
}

/// A before-rule's verdict on a pending event.
#[derive(Clone, Debug, PartialEq)]
pub enum Before {
    Pass,
    /// The same event with different details.
    Change(Event),
    Cancel,
    /// A different event in its place.
    Replace(Event),
    /// Leave the event pending and resolve these first.
    Propose(Vec<Proposal>),
}

/// The interface every rule sits behind, whether it is one of the core D&D
/// rules written in Rust or a trait loaded from a stat block.
pub trait Rule: Send + Sync {
    /// Whether the rule has anything to say about events of this kind. A
    /// committed event stays on the stack while rules that listen to it are
    /// still waiting for their turn, so a rule that answers no here for the
    /// kinds it ignores lets those events leave sooner.
    fn listens(&self, _kind: EventKind) -> bool {
        true
    }

    fn before(&self, _event: &Event, _view: &View) -> Before {
        Before::Pass
    }

    fn after(&self, _entry: &Entry, _view: &View) -> Vec<Proposal> {
        Vec::new()
    }
}

/// A committed event, with what caused it and the scope it was proposed in.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: EventId,
    pub event: Event,
    pub cause: Option<EventId>,
    pub scope: Option<Arc<Scope>>,
}

/// Everything that has happened, in order. Append-only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Log {
    entries: Vec<Entry>,
}

impl Log {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.entries.iter().map(|e| &e.event)
    }

    /// The inputs that produced this log. Pushing them into a fresh engine
    /// with the same setup reproduces it exactly.
    pub fn inputs(&self) -> Vec<Input> {
        self.events()
            .filter_map(|event| match event {
                Event::Rolled { value, .. } => Some(Input::Rolled(*value)),
                Event::Chose { choice, forced: false, .. } => Some(Input::Chose(choice.clone())),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn push(&mut self, event: Event, cause: Option<EventId>, scope: Option<Arc<Scope>>) -> EventId {
        let id = EventId(self.entries.len() as u32);
        self.entries.push(Entry { id, event, cause, scope });
        id
    }
}

impl Index<EventId> for Log {
    type Output = Entry;
    fn index(&self, id: EventId) -> &Entry {
        &self.entries[id.0 as usize]
    }
}

/// Which rule, for remembering that it has already had its turn on an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuleKey {
    Core(u32),
    Trait { owner: Id, index: u32 },
    Effect { owner: Id, effect: u32, index: u32 },
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    Before,
    After,
}

/// A Rust rule and where it sits in the order.
pub(crate) struct Slot {
    pub rule: Box<dyn Rule>,
    pub band: Band,
    /// Among after-rules, run once every other rule has had its turn.
    pub late: bool,
}

pub(crate) enum RuleRef<'a> {
    Rust(&'a dyn Rule),
    Data(TraitRule<'a>),
}

impl RuleRef<'_> {
    pub fn before(&self, event: &Event, view: &View) -> Before {
        match self {
            RuleRef::Rust(rule) => rule.before(event, view),
            RuleRef::Data(rule) => rule.before(event, view),
        }
    }

    pub fn after(&self, entry: &Entry, view: &View) -> Vec<Proposal> {
        match self {
            RuleRef::Rust(rule) => rule.after(entry, view),
            RuleRef::Data(rule) => rule.after(entry, view),
        }
    }
}

pub(crate) struct Candidate<'a> {
    pub key: RuleKey,
    sort: (u8, u32, u32),
    pub rule: RuleRef<'a>,
}

/// What a rule can see. All of it is read-only.
pub struct View<'a> {
    pub state: &'a State,
    pub log: &'a Log,
    pub config: &'a Config,
    /// The proposal being judged, when a before-rule is running.
    pub pending: Option<ProposalId>,
    pub(crate) slots: &'a [Slot],
    pub(crate) stack: &'a [Item],
}

impl<'a> View<'a> {
    /// A creature's armor class right now. This is the `Ac` query: the base
    /// value passes through every before-rule that changes it, and nothing
    /// is committed.
    pub fn ac(&self, who: Id) -> i32 {
        let mut query = Event::Ac { who, value: self.state.creature(who).block.ac as i32 };
        for candidate in self.candidates(Phase::Before, &query) {
            if let Before::Change(next @ Event::Ac { .. }) = candidate.rule.before(&query, self) {
                query = next;
            }
        }
        match query {
            Event::Ac { value, .. } => value,
            _ => unreachable!("an Ac query stays an Ac query"),
        }
    }

    /// A pending event further down the stack.
    pub fn pending_event(&self, id: ProposalId) -> Option<&'a Event> {
        self.stack.iter().find_map(|item| match item {
            Item::Proposed(p) if p.id == id => Some(&p.event),
            _ => None,
        })
    }

    /// Every rule that applies to the event right now, in the order they
    /// take their turns.
    ///
    /// Before: by band, and within a band the Rust rules first, then
    /// creatures in turn order. After: the early Rust rules, then the acting
    /// creature's rules, the target's, everyone else's in turn order, and
    /// the late Rust rules last.
    pub(crate) fn candidates(&self, phase: Phase, event: &Event) -> Vec<Candidate<'a>> {
        let state: &'a State = self.state;
        let kind = event.kind();
        let mut out = Vec::new();
        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.rule.listens(kind) {
                continue;
            }
            let group = match phase {
                Phase::Before => slot.band as u8,
                Phase::After if slot.late => 4,
                Phase::After => 0,
            };
            out.push(Candidate {
                key: RuleKey::Core(i as u32),
                sort: (group, 0, i as u32),
                rule: RuleRef::Rust(&*slot.rule),
            });
        }
        for creature in &state.creatures {
            let owner = creature.id;
            let position = state.order_position(owner) + 1;
            let block_rules =
                creature.block.traits.iter().enumerate().map(|(i, def)| {
                    (RuleKey::Trait { owner, index: i as u32 }, TraitRule { owner, def, source: None })
                });
            let effect_rules = creature.effects.iter().flat_map(|effect| {
                effect.def.rules().enumerate().map(move |(i, def)| {
                    let key = RuleKey::Effect { owner, effect: effect.id, index: i as u32 };
                    (key, TraitRule { owner, def, source: effect.source })
                })
            });
            for (n, (key, rule)) in block_rules.chain(effect_rules).enumerate() {
                let listens = match phase {
                    Phase::Before => rule.def.before,
                    Phase::After => rule.def.after,
                };
                if listens != Some(kind) || !rule.matches(event, self) {
                    continue;
                }
                let group = match phase {
                    Phase::Before => rule.def.band() as u8,
                    Phase::After if event.actor() == Some(owner) => 1,
                    Phase::After if event.target() == Some(owner) => 2,
                    Phase::After => 3,
                };
                out.push(Candidate { key, sort: (group, position, n as u32), rule: RuleRef::Data(rule) });
            }
        }
        out.sort_by_key(|c| c.sort);
        out
    }
}
