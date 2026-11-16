//! The engine: state, a log, an explicit stack, and rules.
//!
//! The engine is passive. It holds no policy and no random number
//! generator. [`Engine::expects`] says what it needs next, a choice or a
//! roll, and [`Engine::push`] supplies it. Everything else that happens is
//! derived: the engine works through its stack until it needs another input
//! or the fight is over.
//!
//! An event's life has four steps. It is proposed. Before-rules take turns
//! with it. It commits, which appends it to the log and applies its built-in
//! effect to the state. After-rules take turns responding to it. An event
//! commits when it is on top of the stack with no before-rule left to run,
//! and its requirement still holds; otherwise it fizzles.

use crate::core_rules::{self, Core, CoreLate};
use crate::data::{Band, Cost, INHERIT, Node, TargetRule};
use crate::dice::{Dice, RollSpec};
use crate::event::{Choice, Constraint, Event, EventId, Expect, Fizzle, Input, Purpose};
use crate::rule::{Before, Log, Phase, Proposal, ProposalId, Rule, RuleKey, Scope, Slot, Step, View};
use crate::state::{Id, Setup, State};
use std::fmt;
use std::sync::Arc;

/// Limits that stop a fight which would never end by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The fight is a draw once this many rounds have been played.
    pub max_rounds: u32,
    /// Stack depth that counts as a runaway chain of rules.
    pub max_stack: usize,
    /// Committed events in one turn that count as a loop.
    pub max_events_per_turn: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { max_rounds: 100, max_stack: 256, max_events_per_turn: 10_000 }
    }
}

/// A limit was hit. The fight ended as an error, not a result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    StackTooDeep,
    TooManyEvents,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Fault::StackTooDeep => {
                "the stack grew past its limit, which means rules are triggering each other without end"
            }
            Fault::TooManyEvents => "one turn committed more events than the limit, which means rules are looping",
        })
    }
}

/// Why an input was refused. A refused input changes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    NothingExpected,
    ExpectedRoll,
    ExpectedChoice,
    OutOfRange { min: u32, max: u32 },
    NotAllowed,
    Faulted(Fault),
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Invalid::NothingExpected => f.write_str("the fight is over; no input is expected"),
            Invalid::ExpectedRoll => f.write_str("a roll is expected, not a choice"),
            Invalid::ExpectedChoice => f.write_str("a choice is expected, not a roll"),
            Invalid::OutOfRange { min, max } => write!(f, "the roll must be between {min} and {max}"),
            Invalid::NotAllowed => f.write_str("that choice does not satisfy the constraint"),
            Invalid::Faulted(fault) => write!(f, "the fight ended in an error: {fault}"),
        }
    }
}

impl std::error::Error for Invalid {}

/// A proposed event waiting on the stack.
pub(crate) struct Pending {
    pub id: ProposalId,
    pub event: Event,
    pub cause: Option<EventId>,
    pub scope: Option<Arc<Scope>>,
    /// Before-rules that have had their turn. A rule sees an event once,
    /// even if another rule changes or replaces it afterwards.
    pub applied: Vec<RuleKey>,
    /// Set when a rule's effect cancelled it, with the event that effect ran
    /// under.
    pub cancelled: Option<Option<EventId>>,
    /// The effect list this event is a cost of. Fizzling abandons that list.
    pub cost_of: Option<u32>,
}

/// What to do with a choice once it arrives, beyond logging it.
pub(crate) enum Then {
    Nothing,
    /// On a yes, run the optional rule's steps.
    React {
        steps: Vec<Step>,
        scope: Arc<Scope>,
    },
    /// Run the named action's effect on the chosen target.
    Use {
        action: String,
        scope: Arc<Scope>,
    },
}

pub(crate) enum Item {
    /// An event before it commits.
    Proposed(Pending),
    /// An event after it commits, while after-rules remain.
    Committed { id: EventId, applied: Vec<RuleKey> },
    /// An effect list and how far through it is.
    Effects { list: u32, steps: Vec<Step>, next: usize, scope: Arc<Scope>, cause: Option<EventId> },
    /// A request for a choice or a roll. The engine stops here.
    Input { expect: Expect, cause: Option<EventId>, scope: Option<Arc<Scope>>, then: Then },
}

pub struct Engine {
    state: State,
    log: Log,
    stack: Vec<Item>,
    slots: Vec<Slot>,
    config: Config,
    fault: Option<Fault>,
    next_id: u32,
    events_this_turn: u32,
}

static DONE: Expect = Expect::Done;

impl Engine {
    /// Starts a fight and runs it up to the first input it needs.
    pub fn new(setup: &Setup) -> Result<Engine, String> {
        Engine::with_rules(setup, Config::default(), Vec::new())
    }

    /// Starts a fight with extra Rust rules alongside the core ones. Their
    /// before-rules run in the first band and their after-rules run with the
    /// early core rules, in the order given.
    pub fn with_rules(setup: &Setup, config: Config, extra: Vec<Box<dyn Rule>>) -> Result<Engine, String> {
        let mut slots = vec![Slot { rule: Box::new(Core), band: Band::Adjust, late: false }];
        slots.extend(extra.into_iter().map(|rule| Slot { rule, band: Band::Adjust, late: false }));
        slots.push(Slot { rule: Box::new(CoreLate), band: Band::Adjust, late: true });
        let mut engine = Engine {
            state: State::new(setup)?,
            log: Log::default(),
            stack: Vec::new(),
            slots,
            config,
            fault: None,
            next_id: 0,
            events_this_turn: 0,
        };
        engine.propose(Event::FightStarted, None, None, None);
        engine.run();
        Ok(engine)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn log(&self) -> &Log {
        &self.log
    }

    /// How many items are waiting on the stack. It stays small however long
    /// the fight runs, because a finished event leaves before its
    /// consequences resolve.
    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }

    /// Set when a loop limit ended the fight.
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }

    /// What the engine needs next: a choice, a roll, or nothing.
    pub fn expects(&self) -> &Expect {
        match self.stack.last() {
            Some(Item::Input { expect, .. }) => expect,
            _ => &DONE,
        }
    }

    /// Every choice that satisfies the constraint the engine is waiting on.
    /// Empty when it is waiting for a roll or for nothing. This is the same
    /// test [`Engine::push`] applies, so a listed choice is always accepted.
    pub fn options(&self) -> Vec<Choice> {
        match self.expects() {
            Expect::Choice { actor, constraint } => self.options_for(*actor, constraint),
            _ => Vec::new(),
        }
    }

    /// Supplies the input the engine is waiting for, then runs until it
    /// needs another. An input that does not fit is refused and changes
    /// nothing.
    pub fn push(&mut self, input: Input) -> Result<(), Invalid> {
        self.check(&input)?;
        let Some(Item::Input { expect, cause, scope, then }) = self.stack.pop() else {
            unreachable!("check found an input request on top");
        };
        match (expect, input) {
            (Expect::Roll { spec, purpose }, Input::Rolled(value)) => {
                self.commit(Event::Rolled { spec, purpose, value }, cause, scope);
            }
            (Expect::Choice { actor, constraint }, Input::Chose(choice)) => {
                self.choose(actor, constraint, choice, false, cause, scope, then);
            }
            _ => unreachable!("check matched the input to the request"),
        }
        self.run();
        Ok(())
    }

    fn check(&self, input: &Input) -> Result<(), Invalid> {
        if let Some(fault) = self.fault {
            return Err(Invalid::Faulted(fault));
        }
        match (self.expects(), input) {
            (Expect::Done, _) => Err(Invalid::NothingExpected),
            (Expect::Roll { spec, .. }, Input::Rolled(value)) => {
                let (min, max) = spec.range();
                if (min..=max).contains(value) { Ok(()) } else { Err(Invalid::OutOfRange { min, max }) }
            }
            (Expect::Choice { actor, constraint }, Input::Chose(choice)) => {
                if self.options_for(*actor, constraint).contains(choice) { Ok(()) } else { Err(Invalid::NotAllowed) }
            }
            (Expect::Roll { .. }, _) => Err(Invalid::ExpectedRoll),
            (Expect::Choice { .. }, _) => Err(Invalid::ExpectedChoice),
        }
    }

    fn options_for(&self, actor: Id, constraint: &Constraint) -> Vec<Choice> {
        let state = &self.state;
        let block = &state.creature(actor).block;
        match constraint {
            Constraint::Step => {
                let mut out = Vec::new();
                for action in &block.actions {
                    if action.cost == Cost::Reaction || !state.affordable(actor, action) {
                        continue;
                    }
                    let act = |targets| Choice::Act { action: action.name.clone(), targets };
                    match action.target {
                        TargetRule::None => out.push(act(Vec::new())),
                        rule => out.extend(state.legal_targets(actor, rule).into_iter().map(|t| act(vec![t]))),
                    }
                }
                out.push(Choice::EndTurn);
                out
            }
            Constraint::React { .. } => vec![Choice::React(true), Choice::React(false)],
            Constraint::Target { action } => {
                let rule = block.action(action).map_or(TargetRule::None, |a| a.target);
                state.legal_targets(actor, rule).into_iter().map(Choice::Target).collect()
            }
        }
    }

    fn view(&self, pending: Option<ProposalId>) -> View<'_> {
        View {
            state: &self.state,
            log: &self.log,
            config: &self.config,
            pending,
            slots: &self.slots,
            stack: &self.stack,
        }
    }

    fn fresh(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn run(&mut self) {
        while self.advance() {}
    }

    fn fail(&mut self, fault: Fault) {
        self.fault = Some(fault);
        self.stack.clear();
    }

    /// Does one unit of work on the top of the stack. Returns false when the
    /// engine has to stop: it needs an input, or nothing is left.
    fn advance(&mut self) -> bool {
        if self.stack.len() > self.config.max_stack {
            self.fail(Fault::StackTooDeep);
        }
        match self.stack.pop() {
            None => false,
            Some(Item::Input { expect, cause, scope, then }) => self.advance_input(expect, cause, scope, then),
            Some(Item::Effects { list, steps, next, scope, cause }) => {
                // A finished list leaves the stack. An unfinished one goes
                // back under whatever its next step puts on top.
                if let Some(step) = steps.get(next).cloned() {
                    self.stack.push(Item::Effects { list, steps, next: next + 1, scope: scope.clone(), cause });
                    self.run_step(step, list, &scope, cause);
                }
                true
            }
            Some(Item::Proposed(pending)) => {
                self.advance_proposed(pending);
                true
            }
            Some(Item::Committed { id, applied }) => {
                self.advance_committed(id, applied);
                true
            }
        }
    }

    fn advance_input(&mut self, expect: Expect, cause: Option<EventId>, scope: Option<Arc<Scope>>, then: Then) -> bool {
        let Expect::Choice { actor, constraint } = &expect else {
            self.stack.push(Item::Input { expect, cause, scope, then });
            return false;
        };
        let actor = *actor;
        // A request can go stale while it waits under other work.
        match constraint {
            Constraint::Step if !self.state.can_act(actor) => {
                // Nothing left to decide: the turn is over.
                self.propose(Event::TurnEnded { who: actor }, cause, scope, None);
                return true;
            }
            Constraint::React { .. } if !self.state.can_act(actor) => return true,
            _ => {}
        }
        let mut options = self.options_for(actor, constraint);
        match options.len() {
            0 => true,
            // One option is no choice. The engine takes it and says so in
            // the log. An optional rule always has two.
            1 => {
                let Expect::Choice { constraint, .. } = expect else { unreachable!() };
                self.choose(actor, constraint, options.remove(0), true, cause, scope, then);
                true
            }
            _ => {
                self.stack.push(Item::Input { expect, cause, scope, then });
                false
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn choose(
        &mut self,
        actor: Id,
        prompt: Constraint,
        choice: Choice,
        forced: bool,
        cause: Option<EventId>,
        scope: Option<Arc<Scope>>,
        then: Then,
    ) {
        // What the choice leads to goes under the choice itself, so rules
        // that respond to the choice run first.
        match (then, &choice) {
            (Then::React { steps, scope }, Choice::React(true)) => {
                let list = self.fresh();
                self.stack.push(Item::Effects { list, steps, next: 0, scope, cause });
            }
            (Then::Use { action, scope }, Choice::Target(target)) => {
                let nodes = self.state.creature(actor).block.action(&action).map(|a| a.effect.clone());
                let steps = nodes.unwrap_or_default().into_iter().map(Step::Node).collect();
                let scope = Arc::new(Scope { target: Some(*target), action: Some(action), ..(*scope).clone() });
                let list = self.fresh();
                self.stack.push(Item::Effects { list, steps, next: 0, scope, cause });
            }
            _ => {}
        }
        self.commit(Event::Chose { actor, prompt, choice, forced }, cause, scope);
    }

    fn advance_proposed(&mut self, mut pending: Pending) {
        if let Some(by) = pending.cancelled {
            return self.fizzle(pending, Fizzle::Cancelled(by));
        }
        // An attack's outcome follows from the roll and the target's armor
        // class, so it is worked out again each time the event comes back
        // to the top. This is how a Shield cast in response turns a hit
        // into a miss.
        if let Some(event) = core_rules::rederive(&pending.event, &self.view(Some(pending.id))) {
            pending.event = event;
        }
        if let Err(why) = core_rules::requirement(&pending.event, &self.state) {
            return self.fizzle(pending, Fizzle::Requirement(why));
        }
        // One before-rule gets its turn per visit, picked against the state
        // as it is now. Rules that pass cost nothing, so keep going past them.
        let view = self.view(Some(pending.id));
        let mut verdict = Before::Pass;
        for candidate in view.candidates(Phase::Before, &pending.event) {
            if pending.applied.contains(&candidate.key) {
                continue;
            }
            pending.applied.push(candidate.key.clone());
            verdict = candidate.rule.before(&pending.event, &view);
            if verdict != Before::Pass {
                break;
            }
        }
        match verdict {
            // Every rule has had its turn and the requirement holds.
            Before::Pass => self.commit(pending.event, pending.cause, pending.scope),
            Before::Change(event) | Before::Replace(event) => {
                pending.event = event;
                self.stack.push(Item::Proposed(pending));
            }
            Before::Cancel => self.fizzle(pending, Fizzle::Cancelled(None)),
            Before::Propose(proposals) => {
                let (cause, scope) = (pending.cause, pending.scope.clone());
                self.stack.push(Item::Proposed(pending));
                self.push_proposals(proposals, cause, scope);
            }
        }
    }

    fn advance_committed(&mut self, id: EventId, mut applied: Vec<RuleKey>) {
        let entry = &self.log[id];
        let view = self.view(None);
        let waiting: Vec<_> =
            view.candidates(Phase::After, &entry.event).into_iter().filter(|c| !applied.contains(&c.key)).collect();
        for (i, candidate) in waiting.iter().enumerate() {
            applied.push(candidate.key.clone());
            let proposals = candidate.rule.after(entry, &view);
            if proposals.is_empty() {
                continue;
            }
            // An input is not a cause. What follows a roll or a choice is
            // caused by whatever asked for it.
            let cause = if entry.event.is_input() { entry.cause } else { Some(id) };
            let scope = entry.scope.clone();
            // The event stays on the stack only while rules are waiting, so
            // a finished event is gone before its consequences run.
            if i + 1 < waiting.len() {
                self.stack.push(Item::Committed { id, applied });
            }
            self.push_proposals(proposals, cause, scope);
            return;
        }
    }

    /// Puts proposals on the stack so that the first resolves first.
    fn push_proposals(&mut self, proposals: Vec<Proposal>, cause: Option<EventId>, scope: Option<Arc<Scope>>) {
        for proposal in proposals.into_iter().rev() {
            match proposal {
                Proposal::Event(event) => self.propose(event, cause, scope.clone(), None),
                Proposal::Steps(steps, own) => {
                    let list = self.fresh();
                    self.stack.push(Item::Effects { list, steps, next: 0, scope: Arc::new(own), cause });
                }
                Proposal::Need(expect) => {
                    self.stack.push(Item::Input { expect, cause, scope: scope.clone(), then: Then::Nothing });
                }
                Proposal::Ask { actor, rule, steps, scope: own } => {
                    let on = own.trigger.as_ref().map_or(Event::FightStarted, |t| t.event.clone());
                    let expect = Expect::Choice { actor, constraint: Constraint::React { rule, on: Box::new(on) } };
                    let then = Then::React { steps, scope: Arc::new(own) };
                    self.stack.push(Item::Input { expect, cause, scope: scope.clone(), then });
                }
            }
        }
    }

    fn propose(&mut self, event: Event, cause: Option<EventId>, scope: Option<Arc<Scope>>, cost_of: Option<u32>) {
        let id = ProposalId(self.fresh());
        self.stack.push(Item::Proposed(Pending {
            id,
            event,
            cause,
            scope,
            applied: Vec::new(),
            cancelled: None,
            cost_of,
        }));
    }

    /// Appends the event to the log and applies its built-in effect. From
    /// here on it has happened.
    fn commit(&mut self, event: Event, cause: Option<EventId>, scope: Option<Arc<Scope>>) {
        if matches!(event, Event::TurnStarted { .. }) {
            self.events_this_turn = 0;
        }
        self.state.apply(&event);
        let ended = matches!(event, Event::FightEnded { .. });
        let id = self.log.push(event, cause, scope);
        if ended {
            // Whatever was still waiting no longer matters.
            self.stack.clear();
        }
        self.stack.push(Item::Committed { id, applied: Vec::new() });
        self.count_event();
    }

    /// Drops a proposed event and logs a marker saying why. Nothing that
    /// already happened is undone.
    fn fizzle(&mut self, pending: Pending, reason: Fizzle) {
        self.log.push(Event::Fizzled { event: Box::new(pending.event), reason }, pending.cause, None);
        if let Some(owner) = pending.cost_of
            && matches!(self.stack.last(), Some(Item::Effects { list, .. }) if *list == owner)
        {
            // An unpaid cost abandons what it was paying for.
            self.stack.pop();
        }
        self.count_event();
    }

    fn count_event(&mut self) {
        self.events_this_turn += 1;
        if self.events_this_turn > self.config.max_events_per_turn {
            self.fail(Fault::TooManyEvents);
        }
    }

    fn pending_mut(&mut self, scope: &Scope) -> Option<&mut Pending> {
        let id = scope.trigger.as_ref()?.pending?;
        self.stack.iter_mut().find_map(|item| match item {
            Item::Proposed(p) if p.id == id => Some(p),
            _ => None,
        })
    }

    fn run_step(&mut self, step: Step, list: u32, scope: &Arc<Scope>, cause: Option<EventId>) {
        match step {
            Step::Spend { who, resource, n } => {
                self.propose(Event::ResourceSpent { who, resource, n }, cause, Some(scope.clone()), Some(list));
            }
            Step::Event(event) => self.propose(event, cause, Some(scope.clone()), None),
            Step::Scoped(steps, own) => self.push_steps(steps, Arc::new(own), cause),
            Step::Change(change) => {
                if let Some(pending) = self.pending_mut(scope) {
                    pending.event = change.apply(&pending.event);
                }
            }
            Step::Cancel => {
                if let Some(pending) = self.pending_mut(scope) {
                    pending.cancelled = Some(cause);
                }
            }
            Step::Node(node) => self.run_node(node, scope, cause),
        }
    }

    fn push_steps(&mut self, steps: Vec<Step>, scope: Arc<Scope>, cause: Option<EventId>) {
        let list = self.fresh();
        self.stack.push(Item::Effects { list, steps, next: 0, scope, cause });
    }

    /// Interprets one effect node. A node never does anything by itself: it
    /// proposes an event, asks for an input, or pushes a nested list.
    fn run_node(&mut self, node: Node, scope: &Arc<Scope>, cause: Option<EventId>) {
        let state = &self.state;
        let env = scope.env(state);
        let actor = scope.actor;
        let aim = |to: &Option<crate::expr::Expr>| match to {
            Some(expr) => expr.creature(&env),
            None => scope.target,
        };
        let as_steps = |nodes: &[Node]| nodes.iter().cloned().map(Step::Node).collect::<Vec<_>>();
        let inherited = Some(scope.clone());
        match node {
            Node::Attack(spec) => {
                let Some(target) = scope.target else { return };
                let action = scope.action.clone().unwrap_or_default();
                let melee = state.creature(actor).block.action(&action).is_some_and(|a| a.target == TargetRule::Melee);
                let event = Event::Attack {
                    attacker: actor,
                    target,
                    action,
                    bonus: spec.bonus,
                    advantage: false,
                    disadvantage: false,
                    melee,
                    spec,
                };
                self.propose(event, cause, inherited, None);
            }
            Node::Save(spec) => {
                let Some(who) = aim(&spec.who) else { return };
                let event = Event::Save {
                    who,
                    ability: spec.ability,
                    dc: spec.dc,
                    bonus: state.creature(who).block.save_bonus(spec.ability),
                    source: Some(actor),
                    advantage: false,
                    disadvantage: false,
                    auto_fail: false,
                    spec,
                };
                self.propose(event, cause, inherited, None);
            }
            Node::Damage(damage) => {
                let Some(target) = aim(&damage.to) else { return };
                let kind = match damage.kind.as_str() {
                    INHERIT => scope.inherit.clone().unwrap_or_else(|| "untyped".to_string()),
                    kind => kind.to_string(),
                };
                let dice = if scope.crit { damage.dice.doubled() } else { damage.dice };
                let purpose =
                    Purpose::Damage { target, kind, source: Some(actor), crit: scope.crit, half: damage.half };
                self.amount(dice, purpose, scope, cause);
            }
            Node::Heal(heal) => {
                let Some(target) = aim(&heal.to) else { return };
                self.amount(heal.dice, Purpose::Heal { target }, scope, cause);
            }
            Node::TempHp(temp) => {
                let Some(target) = aim(&temp.to) else { return };
                self.amount(temp.dice, Purpose::TempHp { target }, scope, cause);
            }
            Node::Apply(effect) => {
                let Some(target) = aim(&effect.to) else { return };
                self.propose(Event::EffectAttached { target, source: Some(actor), effect }, cause, inherited, None);
            }
            Node::Restore(restore) => {
                let n = restore.amount.unwrap_or_else(|| state.max_resource(actor, &restore.resource));
                let event = Event::ResourceRestored { who: actor, resource: restore.resource, n };
                self.propose(event, cause, inherited, None);
            }
            Node::If(branch) => {
                let nodes = if branch.when.test(&env) { &branch.then } else { &branch.otherwise };
                self.push_steps(as_steps(nodes), scope.clone(), cause);
            }
            Node::Use(used) => {
                let Some(action) = state.creature(actor).block.action(&used.action) else { return };
                if used.as_action {
                    // A full use: pay to declare it, then propose ActionUsed,
                    // which others can respond to like any other.
                    let targets = match action.target {
                        TargetRule::None => Vec::new(),
                        TargetRule::Me => vec![actor],
                        rule => match scope.target {
                            Some(t) if state.legal_targets(actor, rule).contains(&t) => vec![t],
                            _ => return,
                        },
                    };
                    let mut steps = Vec::new();
                    if let Some(pool) = action.cost.pool() {
                        steps.push(Step::Spend { who: actor, resource: pool.to_string(), n: 1 });
                    }
                    steps.push(Step::Event(Event::ActionUsed { actor, action: used.action, targets }));
                    self.push_steps(steps, scope.clone(), cause);
                    return;
                }
                // A part of the enclosing action: run the effect directly.
                match action.target {
                    rule @ (TargetRule::None | TargetRule::Me) => {
                        let target = if rule == TargetRule::Me { Some(actor) } else { scope.target };
                        let steps = as_steps(&action.effect);
                        let own = Scope { target, action: Some(used.action), ..(**scope).clone() };
                        self.push_steps(steps, Arc::new(own), cause);
                    }
                    _ => {
                        let expect =
                            Expect::Choice { actor, constraint: Constraint::Target { action: used.action.clone() } };
                        let then = Then::Use { action: used.action, scope: scope.clone() };
                        self.stack.push(Item::Input { expect, cause, scope: inherited, then });
                    }
                }
            }
            Node::Cancel(_) => {
                if let Some(pending) = self.pending_mut(scope) {
                    pending.cancelled = Some(cause);
                }
            }
        }
    }

    /// Proposes whatever an amount of dice is for, asking for a roll first
    /// unless the amount is fixed.
    fn amount(&mut self, dice: Dice, purpose: Purpose, scope: &Arc<Scope>, cause: Option<EventId>) {
        if dice.is_fixed() {
            if let Some(event) = purpose.amount_event(dice.min()) {
                self.propose(event, cause, Some(scope.clone()), None);
            }
            return;
        }
        let expect = Expect::Roll { spec: RollSpec::Sum(dice), purpose };
        self.stack.push(Item::Input { expect, cause, scope: Some(scope.clone()), then: Then::Nothing });
    }
}
