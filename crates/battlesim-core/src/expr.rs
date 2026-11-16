//! The small expression language used in `when`, `who` and `to`.
//!
//! It has comparisons, `and`/`or`/`not`, whole-number arithmetic, a fixed set
//! of subjects (`self`, `target`, `attacker`, `actor`, `source`, `action`,
//! `event`) and a fixed set of functions. It has no loops, no variables and
//! no way to define anything. Names are checked when the expression loads.

use crate::data::Size;
use crate::event::Event;
use crate::state::{Id, Life, Line, State};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Debug, PartialEq)]
enum Ast {
    Int(i64),
    Bool(bool),
    Path(Vec<String>),
    Call(String, Vec<Ast>),
    Not(Box<Ast>),
    And(Box<Ast>, Box<Ast>),
    Or(Box<Ast>, Box<Ast>),
    Cmp(&'static str, Box<Ast>, Box<Ast>),
    Add(Box<Ast>, Box<Ast>),
    Sub(Box<Ast>, Box<Ast>),
}

/// A parsed, name-checked expression.
#[derive(Clone, Debug)]
pub struct Expr {
    src: String,
    ast: Ast,
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.src == other.src
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    /// A bare word such as `prone`, `fire` or `finesse`.
    Sym(String),
    Creature(Id),
    /// An action, as its owner and name.
    Action(Id, String),
}

/// What an expression is evaluated against.
pub struct Env<'a> {
    pub state: &'a State,
    /// What `self` means.
    pub me: Id,
    /// What `target` means, when the event does not say.
    pub target: Option<Id>,
    pub event: Option<&'a Event>,
    /// What `action` means, when the event does not say.
    pub action: Option<(Id, &'a str)>,
    /// What `source` means, when the event does not say.
    pub source: Option<Id>,
}

const CREATURE_SUBJECTS: &[&str] = &["self", "target", "attacker", "actor", "source"];
const SUBJECTS: &[&str] = &["self", "target", "attacker", "actor", "source", "action", "event"];
const CREATURE_FIELDS: &[&str] = &["size", "hp", "max_hp", "temp_hp", "bloodied", "alive", "side", "line"];
const EVENT_CREATURE_FIELDS: &[&str] = &["actor", "attacker", "target", "source", "who"];
const EVENT_FIELDS: &[&str] = &[
    "actor",
    "attacker",
    "target",
    "source",
    "who",
    "action",
    "advantage",
    "disadvantage",
    "crit",
    "melee",
    "auto_fail",
    "amount",
    "natural",
    "total",
    "dc",
    "n",
    "round",
    "bonus",
    "type",
    "ability",
    "resource",
];

#[derive(Copy, Clone, PartialEq)]
enum Arg {
    Creature,
    Action,
    Sym,
}

/// Every function, with the argument lists it accepts.
const FUNCTIONS: &[(&str, &[&[Arg]])] = &[
    ("has_condition", &[&[Arg::Creature, Arg::Sym]]),
    ("has_tag", &[&[Arg::Action, Arg::Sym]]),
    ("engaged", &[&[Arg::Creature], &[Arg::Creature, Arg::Creature]]),
    ("ally_engaged_with", &[&[Arg::Creature]]),
    ("is_enemy", &[&[Arg::Creature]]),
    ("is_ally", &[&[Arg::Creature]]),
    ("resource", &[&[Arg::Sym], &[Arg::Creature, Arg::Sym]]),
    ("round", &[&[]]),
];

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    LParen,
    RParen,
    Comma,
    Dot,
    Op(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let (tok, len) = match c {
            c if c.is_whitespace() => {
                i += 1;
                continue;
            }
            '(' => (Tok::LParen, 1),
            ')' => (Tok::RParen, 1),
            ',' => (Tok::Comma, 1),
            '.' => (Tok::Dot, 1),
            '+' => (Tok::Op("+"), 1),
            '-' => (Tok::Op("-"), 1),
            '=' if next == Some('=') => (Tok::Op("=="), 2),
            '!' if next == Some('=') => (Tok::Op("!="), 2),
            '<' if next == Some('=') => (Tok::Op("<="), 2),
            '>' if next == Some('=') => (Tok::Op(">="), 2),
            '<' => (Tok::Op("<"), 1),
            '>' => (Tok::Op(">"), 1),
            c if c.is_ascii_digit() => {
                let end = (i..chars.len()).find(|&j| !chars[j].is_ascii_digit()).unwrap_or(chars.len());
                let text: String = chars[i..end].iter().collect();
                (Tok::Int(text.parse().map_err(|_| format!("number `{text}` is too large"))?), end - i)
            }
            c if c.is_alphabetic() || c == '_' => {
                let end =
                    (i..chars.len()).find(|&j| !(chars[j].is_alphanumeric() || chars[j] == '_')).unwrap_or(chars.len());
                (Tok::Ident(chars[i..end].iter().collect()), end - i)
            }
            other => return Err(format!("unexpected character `{other}`")),
        };
        out.push(tok);
        i += len;
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        let hit = self.peek() == Some(tok);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn eat_word(&mut self, word: &str) -> bool {
        self.eat(&Tok::Ident(word.to_string()))
    }

    fn or(&mut self) -> Result<Ast, String> {
        let mut left = self.and()?;
        while self.eat_word("or") {
            left = Ast::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Ast, String> {
        let mut left = self.not()?;
        while self.eat_word("and") {
            left = Ast::And(Box::new(left), Box::new(self.not()?));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Ast, String> {
        if self.eat_word("not") { Ok(Ast::Not(Box::new(self.not()?))) } else { self.cmp() }
    }

    fn cmp(&mut self) -> Result<Ast, String> {
        let left = self.sum()?;
        match self.peek() {
            Some(&Tok::Op(op)) if op != "+" && op != "-" => {
                self.pos += 1;
                Ok(Ast::Cmp(op, Box::new(left), Box::new(self.sum()?)))
            }
            _ => Ok(left),
        }
    }

    fn sum(&mut self) -> Result<Ast, String> {
        let mut left = self.atom()?;
        loop {
            if self.eat(&Tok::Op("+")) {
                left = Ast::Add(Box::new(left), Box::new(self.atom()?));
            } else if self.eat(&Tok::Op("-")) {
                left = Ast::Sub(Box::new(left), Box::new(self.atom()?));
            } else {
                return Ok(left);
            }
        }
    }

    fn atom(&mut self) -> Result<Ast, String> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Ast::Int(n)),
            Some(Tok::Op("-")) => match self.next() {
                Some(Tok::Int(n)) => Ok(Ast::Int(-n)),
                _ => Err("expected a number after `-`".into()),
            },
            Some(Tok::LParen) => {
                let inner = self.or()?;
                if !self.eat(&Tok::RParen) {
                    return Err("missing `)`".into());
                }
                Ok(inner)
            }
            Some(Tok::Ident(name)) => match name.as_str() {
                "true" => Ok(Ast::Bool(true)),
                "false" => Ok(Ast::Bool(false)),
                "and" | "or" | "not" => Err(format!("`{name}` is not expected here")),
                _ if self.eat(&Tok::LParen) => {
                    let mut args = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            args.push(self.or()?);
                            if self.eat(&Tok::RParen) {
                                break;
                            }
                            if !self.eat(&Tok::Comma) {
                                return Err(format!("expected `,` or `)` in the arguments of `{name}`"));
                            }
                        }
                    }
                    Ok(Ast::Call(name, args))
                }
                _ => {
                    let mut path = vec![name];
                    while self.eat(&Tok::Dot) {
                        match self.next() {
                            Some(Tok::Ident(field)) => path.push(field),
                            _ => return Err("expected a name after `.`".into()),
                        }
                    }
                    Ok(Ast::Path(path))
                }
            },
            Some(other) => Err(format!("unexpected {other:?}")),
            None => Err("the expression ends too soon".into()),
        }
    }
}

fn one_of(names: &[&str]) -> String {
    names.join(", ")
}

fn is_creature(ast: &Ast) -> bool {
    match ast {
        Ast::Path(p) if p.len() == 1 => CREATURE_SUBJECTS.contains(&p[0].as_str()),
        Ast::Path(p) if p.len() == 2 => p[0] == "event" && EVENT_CREATURE_FIELDS.contains(&p[1].as_str()),
        _ => false,
    }
}

fn check(ast: &Ast) -> Result<(), String> {
    match ast {
        Ast::Int(_) | Ast::Bool(_) => Ok(()),
        Ast::Not(a) => check(a),
        Ast::And(a, b) | Ast::Or(a, b) | Ast::Cmp(_, a, b) | Ast::Add(a, b) | Ast::Sub(a, b) => {
            check(a)?;
            check(b)
        }
        Ast::Path(path) => check_path(path),
        Ast::Call(name, args) => {
            let Some((_, forms)) = FUNCTIONS.iter().find(|(n, _)| n == name) else {
                let names: Vec<&str> = FUNCTIONS.iter().map(|(n, _)| *n).collect();
                return Err(format!("unknown function `{name}`; functions are {}", one_of(&names)));
            };
            let Some(form) = forms.iter().find(|f| f.len() == args.len()) else {
                return Err(format!("`{name}` does not take {} argument(s)", args.len()));
            };
            for (arg, want) in args.iter().zip(form.iter()) {
                let ok = match want {
                    Arg::Creature => is_creature(arg),
                    Arg::Action => {
                        matches!(arg, Ast::Path(p) if p[..] == ["action"] || p[..] == ["event", "action"])
                    }
                    Arg::Sym => matches!(arg, Ast::Path(p) if p.len() == 1 && !SUBJECTS.contains(&p[0].as_str())),
                };
                if !ok {
                    return Err(match want {
                        Arg::Creature => format!(
                            "`{name}` needs a creature here: one of {}, or an event field such as `event.actor`",
                            one_of(CREATURE_SUBJECTS)
                        ),
                        Arg::Action => format!("`{name}` needs `action` or `event.action` here"),
                        Arg::Sym => format!("`{name}` needs a plain name here, such as `prone`"),
                    });
                }
            }
            Ok(())
        }
    }
}

fn check_path(path: &[String]) -> Result<(), String> {
    let head = path[0].as_str();
    if path.len() == 1 {
        // A lone word is a subject or a plain name such as `fire` or `large`.
        return Ok(());
    }
    let field = |name: &str, known: &[&str], of: &str| {
        if known.contains(&name) {
            Ok(())
        } else {
            Err(format!("unknown {of} field `{name}`; fields are {}", one_of(known)))
        }
    };
    match head {
        "event" => {
            field(&path[1], EVENT_FIELDS, "event")?;
            match path.len() {
                2 => Ok(()),
                3 if EVENT_CREATURE_FIELDS.contains(&path[1].as_str()) => field(&path[2], CREATURE_FIELDS, "creature"),
                _ => Err(format!("`{}` goes too deep", path.join("."))),
            }
        }
        "action" => Err("`action` has no fields; use `has_tag(action, ...)`".into()),
        _ if CREATURE_SUBJECTS.contains(&head) => {
            if path.len() > 2 {
                return Err(format!("`{}` goes too deep", path.join(".")));
            }
            field(&path[1], CREATURE_FIELDS, "creature")
        }
        _ => Err(format!("unknown name `{head}`; a dotted name starts with one of {}", one_of(SUBJECTS))),
    }
}

fn creature_field(state: &State, id: Id, field: &str) -> Option<Value> {
    let c = state.creature(id);
    Some(match field {
        "size" => Value::Int(c.block.size as i64),
        "hp" => Value::Int(c.hp.into()),
        "max_hp" => Value::Int(c.block.hp.into()),
        "temp_hp" => Value::Int(c.temp_hp.into()),
        "bloodied" => Value::Bool(c.hp * 2 <= c.block.hp),
        "alive" => Value::Bool(c.life == Life::Alive),
        "side" => Value::Sym(c.side.clone()),
        "line" => Value::Sym(if c.line == Line::Front { "front" } else { "back" }.into()),
        _ => return None,
    })
}

fn event_field(event: &Event, field: &str) -> Option<Value> {
    use Event::*;
    Some(match field {
        "actor" | "attacker" => Value::Creature(event.actor()?),
        "target" => Value::Creature(event.target()?),
        "source" => Value::Creature(event.source()?),
        "who" => Value::Creature(event.subject()?),
        "action" => {
            let (owner, name) = event.action_ref()?;
            Value::Action(owner, name.to_string())
        }
        "advantage" => Value::Bool(match event {
            Attack { advantage, disadvantage, .. } | Save { advantage, disadvantage, .. } => {
                *advantage && !*disadvantage
            }
            Hit(o) | Miss(o) => o.advantage,
            _ => return None,
        }),
        "disadvantage" => Value::Bool(match event {
            Attack { advantage, disadvantage, .. } | Save { advantage, disadvantage, .. } => {
                *disadvantage && !*advantage
            }
            Hit(o) | Miss(o) => o.disadvantage,
            _ => return None,
        }),
        "crit" => Value::Bool(match event {
            Hit(o) => o.crit,
            Damage { crit, .. } => *crit,
            _ => return None,
        }),
        "melee" => Value::Bool(match event {
            Attack { melee, .. } => *melee,
            Hit(o) | Miss(o) => o.melee,
            _ => return None,
        }),
        "auto_fail" => match event {
            Save { auto_fail, .. } => Value::Bool(*auto_fail),
            _ => return None,
        },
        "amount" => Value::Int(match event {
            Damage { amount, .. } | Heal { amount, .. } | TempHp { amount, .. } => (*amount).into(),
            _ => return None,
        }),
        "natural" => Value::Int(match event {
            Hit(o) | Miss(o) => o.natural.into(),
            SaveFailed { natural, .. } | SaveSucceeded { natural, .. } => (*natural).into(),
            _ => return None,
        }),
        "total" => Value::Int(match event {
            Hit(o) | Miss(o) => o.total.into(),
            SaveFailed { total, .. } | SaveSucceeded { total, .. } => (*total).into(),
            _ => return None,
        }),
        "dc" => Value::Int(match event {
            Save { dc, .. } | SaveFailed { dc, .. } | SaveSucceeded { dc, .. } => (*dc).into(),
            _ => return None,
        }),
        "n" => match event {
            ResourceSpent { n, .. } | ResourceRestored { n, .. } => Value::Int((*n).into()),
            _ => return None,
        },
        "round" => match event {
            RoundStarted { round } => Value::Int((*round).into()),
            _ => return None,
        },
        "bonus" => match event {
            Attack { bonus, .. } | Save { bonus, .. } => Value::Int((*bonus).into()),
            _ => return None,
        },
        "type" => match event {
            Damage { kind, .. } => Value::Sym(kind.clone()),
            _ => return None,
        },
        "ability" => match event {
            Save { ability, .. } => Value::Sym(ability.name().into()),
            _ => return None,
        },
        "resource" => match event {
            ResourceSpent { resource, .. } | ResourceRestored { resource, .. } => Value::Sym(resource.clone()),
            _ => return None,
        },
        _ => return None,
    })
}

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, String> {
        let fail = |why: String| format!("in `{src}`: {why}");
        let mut parser = Parser { toks: lex(src).map_err(fail)?, pos: 0 };
        let ast = parser.or().map_err(fail)?;
        if let Some(extra) = parser.peek() {
            return Err(fail(format!("unexpected {extra:?} after the end of the expression")));
        }
        check(&ast).map_err(fail)?;
        Ok(Expr { src: src.to_string(), ast })
    }

    pub fn source(&self) -> &str {
        &self.src
    }

    pub fn eval(&self, env: &Env) -> Result<Value, String> {
        eval(&self.ast, env)
    }

    /// True only when the expression evaluates to true. One that cannot be
    /// evaluated, because the event has no target for example, is false.
    pub fn test(&self, env: &Env) -> bool {
        self.eval(env) == Ok(Value::Bool(true))
    }

    /// The creature the expression names, if it names one.
    pub fn creature(&self, env: &Env) -> Option<Id> {
        match self.eval(env) {
            Ok(Value::Creature(id)) => Some(id),
            _ => None,
        }
    }
}

fn eval(ast: &Ast, env: &Env) -> Result<Value, String> {
    let int = |a: &Ast| match eval(a, env)? {
        Value::Int(n) => Ok(n),
        other => Err(format!("expected a number, found {other:?}")),
    };
    let truth = |a: &Ast| match eval(a, env)? {
        Value::Bool(b) => Ok(b),
        other => Err(format!("expected true or false, found {other:?}")),
    };
    Ok(match ast {
        Ast::Int(n) => Value::Int(*n),
        Ast::Bool(b) => Value::Bool(*b),
        Ast::Not(a) => Value::Bool(!truth(a)?),
        Ast::And(a, b) => Value::Bool(truth(a)? && truth(b)?),
        Ast::Or(a, b) => Value::Bool(truth(a)? || truth(b)?),
        Ast::Add(a, b) => Value::Int(int(a)? + int(b)?),
        Ast::Sub(a, b) => Value::Int(int(a)? - int(b)?),
        Ast::Cmp(op, a, b) => {
            let (left, right) = (eval(a, env)?, eval(b, env)?);
            Value::Bool(match (*op, &left, &right) {
                ("==", _, _) => left == right,
                ("!=", _, _) => left != right,
                ("<", Value::Int(l), Value::Int(r)) => l < r,
                ("<=", Value::Int(l), Value::Int(r)) => l <= r,
                (">", Value::Int(l), Value::Int(r)) => l > r,
                (">=", Value::Int(l), Value::Int(r)) => l >= r,
                _ => return Err(format!("cannot order {left:?} and {right:?}")),
            })
        }
        Ast::Path(path) => eval_path(path, env)?,
        Ast::Call(name, args) => eval_call(name, args, env)?,
    })
}

fn eval_path(path: &[String], env: &Env) -> Result<Value, String> {
    let head = path[0].as_str();
    let creature = |id: Option<Id>| id.map(Value::Creature).ok_or_else(|| format!("there is no `{head}` here"));
    let (mut value, rest) = match head {
        "event" => {
            let event = env.event.ok_or("there is no `event` here")?;
            let field = path.get(1).ok_or("`event` needs a field")?;
            let value = event_field(event, field).ok_or_else(|| format!("this event has no `{field}`"))?;
            (value, &path[2..])
        }
        "self" => (Value::Creature(env.me), &path[1..]),
        "target" => (creature(env.target.or_else(|| env.event?.target()))?, &path[1..]),
        "attacker" | "actor" => (creature(env.event.and_then(Event::actor))?, &path[1..]),
        "source" => (creature(env.source.or_else(|| env.event?.source()))?, &path[1..]),
        "action" => {
            let (owner, name) = env.action.or_else(|| env.event?.action_ref()).ok_or("there is no `action` here")?;
            (Value::Action(owner, name.to_string()), &path[1..])
        }
        word => {
            return Ok(match Size::NAMES.iter().position(|s| *s == word) {
                Some(rank) => Value::Int(rank as i64),
                None => Value::Sym(word.to_string()),
            });
        }
    };
    for field in rest {
        value = match value {
            Value::Creature(id) => {
                creature_field(env.state, id, field).ok_or_else(|| format!("a creature has no `{field}`"))?
            }
            other => return Err(format!("{other:?} has no `{field}`")),
        };
    }
    Ok(value)
}

fn eval_call(name: &str, args: &[Ast], env: &Env) -> Result<Value, String> {
    let state = env.state;
    let creature = |i: usize| match eval(&args[i], env)? {
        Value::Creature(id) => Ok(id),
        other => Err(format!("`{name}` expected a creature, found {other:?}")),
    };
    let word = |i: usize| match &args[i] {
        Ast::Path(p) if p.len() == 1 => Ok(p[0].as_str()),
        _ => Err(format!("`{name}` expected a plain name")),
    };
    Ok(match (name, args.len()) {
        ("has_condition", 2) => Value::Bool(state.creature(creature(0)?).has_condition(word(1)?)),
        ("has_tag", 2) => match eval(&args[0], env)? {
            Value::Action(owner, action) => {
                let tag = word(1)?;
                let def = state.creature(owner).block.action(&action);
                Value::Bool(def.is_some_and(|a| a.tags.iter().any(|t| t == tag)))
            }
            other => return Err(format!("`has_tag` expected an action, found {other:?}")),
        },
        ("engaged", 1) => {
            let who = creature(0)?;
            Value::Bool(state.engaged_with(who).any(|other| state.is_enemy(who, other)))
        }
        ("engaged", 2) => Value::Bool(state.engaged(creature(0)?, creature(1)?)),
        // An ally of `self`, other than `self`, that is in melee with the
        // creature and able to act. This is what Sneak Attack and Pack
        // Tactics ask for.
        ("ally_engaged_with", 1) => {
            let who = creature(0)?;
            Value::Bool(state.engaged_with(who).any(|o| o != env.me && !state.is_enemy(env.me, o) && state.can_act(o)))
        }
        ("is_enemy", 1) => Value::Bool(state.is_enemy(env.me, creature(0)?)),
        ("is_ally", 1) => {
            let who = creature(0)?;
            Value::Bool(who != env.me && !state.is_enemy(env.me, who))
        }
        ("resource", 1) => Value::Int(state.creature(env.me).resource(word(0)?).into()),
        ("resource", 2) => Value::Int(state.creature(creature(0)?).resource(word(1)?).into()),
        ("round", 0) => Value::Int(state.round.into()),
        _ => return Err(format!("unknown function `{name}`")),
    })
}

impl Serialize for Expr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.src)
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let src = String::deserialize(d)?;
        Expr::parse(&src).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_doc_examples() {
        for src in [
            "ally_engaged_with(target)",
            "has_tag(action, finesse) and (event.advantage or ally_engaged_with(target))",
            "is_enemy(event.actor) and has_tag(event.action, spell)",
            "target.size <= large and not has_condition(target, prone)",
            "event.ability == str or event.ability == dex",
            "resource(spell_slot_1) >= 1 and round() > 1",
            "event.total - 5 < 15",
        ] {
            Expr::parse(src).unwrap();
        }
    }

    #[test]
    fn unknown_names_are_load_errors_that_list_what_is_valid() {
        let cases = [
            ("has_conditon(target, prone)", "unknown function `has_conditon`; functions are has_condition"),
            ("targt.size > 2", "unknown name `targt`"),
            ("target.sise > 2", "unknown creature field `sise`; fields are size"),
            ("event.advntage", "unknown event field `advntage`"),
            ("has_condition(tagret, prone)", "needs a creature here"),
            ("has_tag(target, finesse)", "needs `action` or `event.action`"),
            ("engaged()", "does not take 0 argument(s)"),
            ("target.size <", "ends too soon"),
            ("a = b", "unexpected character `=`"),
            ("(a == b", "missing `)`"),
            ("a == b c", "after the end"),
        ];
        for (src, want) in cases {
            let err = Expr::parse(src).unwrap_err();
            assert!(err.contains(want), "`{src}` gave `{err}`, which lacks `{want}`");
        }
    }
}
