use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::path::is_separator;
use std::process::id;
use pest::iterators::{Pair, Pairs};
use cozorocks::{SlicePtr, StatusCode};
use crate::db::engine::{Session};
use crate::relation::table::{Table};
use crate::relation::tuple::{OwnTuple, Tuple};
use crate::relation::typing::Typing;
use crate::relation::value::Value;
use crate::error::{CozoError, Result};
use crate::relation::data::DataKind;
use crate::parser::Rule;
use crate::parser::text_identifier::build_name_in_def;
use crate::relation::value;

pub trait Environment<T: AsRef<[u8]>> where Self: Sized {
    fn get_next_storage_id(&mut self, in_root: bool) -> Result<u32>;
    fn get_stack_depth(&self) -> i32;
    fn push_env(&mut self);
    fn pop_env(&mut self) -> Result<()>;
    fn define_variable(&mut self, name: &str, val: &Value, in_root: bool) -> Result<()> {
        let mut data = Tuple::with_data_prefix(DataKind::Value);
        data.push_value(val);
        self.define_data(name, data, in_root)
    }
    fn define_table(&mut self, table: &Table, in_root: bool) -> Result<()> {
        todo!()
    }
    fn resolve(&self, name: &str) -> Result<Option<Tuple<T>>>;
    fn delete_defined(&mut self, name: &str, in_root: bool) -> Result<()>;
    fn define_data(&mut self, name: &str, data: OwnTuple, in_root: bool) -> Result<()>;
    fn encode_definable_key(&self, name: &str, in_root: bool) -> OwnTuple {
        let depth_code = if in_root { 0 } else { self.get_stack_depth() as i64 };
        let mut tuple = Tuple::with_null_prefix();
        tuple.push_str(name);
        tuple.push_int(depth_code);
        tuple
    }
    fn parse_cols(&self, pair: Pair<Rule>) -> Result<(Typing, Typing)> {
        let col_res = pair.into_inner().map(|p| {
            let mut ps = p.into_inner();
            let mut name_ps = ps.next().unwrap().into_inner();
            let is_key;
            let mut name_p = name_ps.next().unwrap();
            match name_p.as_rule() {
                Rule::key_marker => {
                    is_key = true;
                    name_p = name_ps.next().unwrap();
                }
                _ => { is_key = false }
            }
            let name = build_name_in_def(name_p, true)?;
            let type_p = Typing::from_pair(ps.next().unwrap(), Some(self))?;
            Ok((is_key, name, type_p))
        }).collect::<Result<Vec<_>>>()?;
        let all_names = col_res.iter().map(|(_, n, _)| n).collect::<HashSet<_>>();
        if all_names.len() != col_res.len() {
            return Err(CozoError::DuplicateNames(col_res.iter().map(|(_, n, _)| n.to_string()).collect::<Vec<_>>()));
        }
        let (keys, cols): (Vec<_>, Vec<_>) = col_res.iter().partition(|(is_key, _, _)| *is_key);
        let keys_typing = Typing::NamedTuple(keys.iter().map(|(_, n, t)| (n.to_string(), t.clone())).collect());
        let vals_typing = Typing::NamedTuple(cols.iter().map(|(_, n, t)| (n.to_string(), t.clone())).collect());
        Ok((keys_typing, vals_typing))
    }
    fn parse_definition(&self, pair: Pair<Rule>, in_root: bool) -> Result<(bool, (String, OwnTuple))> {
        Ok(match pair.as_rule() {
            Rule::node_def => (true, self.parse_node_def(pair.into_inner(), in_root)?),
            Rule::edge_def => (true, self.parse_edge_def(pair.into_inner(), in_root)?),
            Rule::associate_def => (true, self.parse_assoc_def(pair.into_inner(), in_root)?),
            Rule::index_def => todo!(),
            Rule::type_def => (false, self.parse_type_def(pair.into_inner(), in_root)?),
            _ => unreachable!()
        })
    }
    fn parse_assoc_def(&self, mut pairs: Pairs<Rule>, in_root: bool) -> Result<(String, OwnTuple)> {
        let name = build_name_in_def(pairs.next().unwrap(), true)?;
        let src_name = build_name_in_def(pairs.next().unwrap(), true)?;
        let src_tbl = match self.resolve(&src_name)? {
            Some(res) => res,
            None => return Err(CozoError::UndefinedType(src_name))
        };
        let (_kind, src_global, src_id) = Self::extract_table_id(src_tbl)?;
        if in_root && !src_global {
            return Err(CozoError::LogicError("Cannot have global edge with local nodes".to_string()));
        }

        let (keys_typing, vals_typing) = self.parse_cols(pairs.next().unwrap())?;
        if keys_typing.to_string() != "()" {
            return Err(CozoError::LogicError("Cannot have keys in assoc".to_string()));
        }
        let mut tuple = Tuple::with_data_prefix(DataKind::Associate);
        tuple.push_bool(src_global);
        tuple.push_uint(src_id);
        tuple.push_str(vals_typing.to_string());
        Ok((name, tuple))
    }
    fn parse_type_def(&self, mut pairs: Pairs<Rule>, _in_root: bool) -> Result<(String, OwnTuple)> {
        let name = build_name_in_def(pairs.next().unwrap(), true)?;
        let typ = Typing::from_pair(pairs.next().unwrap(), Some(self))?;
        let mut data = Tuple::with_data_prefix(DataKind::TypeAlias);
        data.push_str(typ.to_string());
        Ok((name, data))
    }
    fn parse_edge_def(&self, mut pairs: Pairs<Rule>, in_root: bool) -> Result<(String, OwnTuple)> {
        let src_name = build_name_in_def(pairs.next().unwrap(), true)?;
        let src_tbl = match self.resolve(&src_name)? {
            Some(res) => res,
            None => return Err(CozoError::UndefinedType(src_name))
        };
        let (kind, src_global, src_id) = Self::extract_table_id(src_tbl)?;
        if in_root && !src_global {
            return Err(CozoError::LogicError("Cannot have global edge with local nodes".to_string()));
        }
        if kind != DataKind::Node {
            return Err(CozoError::UnexpectedDataKind(kind));
        }
        let name = build_name_in_def(pairs.next().unwrap(), true)?;
        let dst_name = build_name_in_def(pairs.next().unwrap(), true)?;
        let dst_tbl = match self.resolve(&dst_name)? {
            Some(res) => res,
            None => return Err(CozoError::UndefinedType(dst_name))
        };
        let (kind, dst_global, dst_id) = Self::extract_table_id(dst_tbl)?;
        if in_root && !dst_global {
            return Err(CozoError::LogicError("Cannot have global edge with local nodes".to_string()));
        }
        if kind != DataKind::Node {
            return Err(CozoError::UnexpectedDataKind(kind));
        }
        let (keys_typing, vals_typing) = match pairs.next() {
            Some(p) => self.parse_cols(p)?,
            None => (Typing::NamedTuple(vec![]), Typing::NamedTuple(vec![]))
        };
        let mut tuple = Tuple::with_data_prefix(DataKind::Edge);
        tuple.push_bool(src_global);
        tuple.push_uint(src_id);
        tuple.push_bool(dst_global);
        tuple.push_uint(dst_id);
        tuple.push_str(keys_typing.to_string());
        tuple.push_str(vals_typing.to_string());
        tuple.push_null(); // TODO default values for keys
        tuple.push_null(); // TODO default values for cols
        Ok((name, tuple))
    }

    fn extract_table_id(src_tbl: Tuple<T>) -> Result<(DataKind, bool, u64)> {
        let kind = src_tbl.data_kind()?;
        match kind {
            DataKind::DataTuple | DataKind::Value | DataKind::TypeAlias => return Err(CozoError::UnexpectedDataKind(kind)),
            _ => {}
        };
        let is_global = match src_tbl.get(0).expect("Data corrupt") {
            Value::Bool(u) => u,
            _ => panic!("Data corrupt")
        };
        let table_id = match src_tbl.get(1).expect("Data corrupt") {
            Value::UInt(u) => u,
            _ => panic!("Data corrupt")
        };
        Ok((kind, is_global, table_id))
    }
    fn parse_node_def(&self, mut pairs: Pairs<Rule>, _in_root: bool) -> Result<(String, OwnTuple)> {
        let name = build_name_in_def(pairs.next().unwrap(), true)?;
        let col_pair = pairs.next().unwrap();
        let (keys_typing, vals_typing) = self.parse_cols(col_pair)?;
        let mut tuple = Tuple::with_data_prefix(DataKind::Node);
        tuple.push_str(keys_typing.to_string());
        tuple.push_str(vals_typing.to_string());
        tuple.push_null(); // TODO default values for keys
        tuple.push_null(); // TODO default values for cols
        Ok((name, tuple))
    }
    fn run_definition(&mut self, pair: Pair<Rule>) -> Result<()> {
        let in_root = match pair.as_rule() {
            Rule::global_def => true,
            Rule::local_def => false,
            _ => unreachable!()
        };

        let (need_id, (name, mut tuple)) = self.parse_definition(
            pair.into_inner().next().unwrap(), in_root,
        )?;
        if need_id {
            let id = self.get_next_storage_id(in_root)?;
            let mut new_tuple = Tuple::with_prefix(tuple.get_prefix());
            new_tuple.push_bool(in_root);
            new_tuple.push_uint(id as u64);
            new_tuple.concat_data(&tuple);
            tuple = new_tuple;
        }
        self.define_data(&name, tuple, in_root)
    }
    fn partial_eval<'a>(&self, value: Value<'a>) -> Result<(bool, Value<'a>)> {
        match value {
            v @ (Value::Null |
            Value::Bool(_) |
            Value::UInt(_) |
            Value::Int(_) |
            Value::Float(_) |
            Value::Uuid(_) |
            Value::Text(_) |
            Value::EndSentinel) => Ok((true, v)),
            Value::List(l) => {
                let init_vec = Vec::with_capacity(l.len());
                let res: Result<(bool, Vec<Value>)> = l.into_iter()
                    .try_fold((true, init_vec), |(is_evaluated, mut accum), val| {
                        let (ev, new_val) = self.partial_eval(val)?;
                        accum.push(new_val);
                        Ok((ev && is_evaluated, accum))
                    });
                let (is_ev, v) = res?;
                Ok((is_ev, v.into()))
            }
            Value::Dict(d) => {
                let res: Result<(bool, BTreeMap<Cow<str>, Value>)> = d.into_iter()
                    .try_fold((true, BTreeMap::new()), |(is_evaluated, mut accum), (k, v)| {
                        let (ev, new_val) = self.partial_eval(v)?;
                        accum.insert(k, new_val);
                        Ok((ev && is_evaluated, accum))
                    });
                let (is_ev, v) = res?;
                Ok((is_ev, v.into()))
            }
            Value::Variable(v) => {
                Ok(match self.resolve(&v)? {
                    None => (false, Value::Variable(v)),
                    Some(rs) => {
                        match rs.data_kind() {
                            Ok(DataKind::Value) => {
                                let resolved = rs.get(0).ok_or_else(|| CozoError::BadDataFormat(rs.data.as_ref().to_vec()))?;
                                (resolved.is_evaluated(), resolved.to_static())
                            }
                            _ => (false, Value::Variable(v))
                        }
                    }
                })
            }
            Value::Apply(op, args) => {
                use crate::relation::value;
                Ok(match op.as_ref() {
                    value::OP_ADD => self.add_values(args)?,
                    value::OP_SUB => self.sub_values(args)?,
                    value::OP_MUL => self.mul_values(args)?,
                    value::OP_DIV => self.div_values(args)?,
                    value::OP_EQ => self.eq_values(args)?,
                    value::OP_NE => self.ne_values(args)?,
                    value::OP_OR => self.or_values(args)?,
                    value::OP_AND => self.and_values(args)?,
                    value::OP_MOD => self.mod_values(args)?,
                    value::OP_GT => self.gt_values(args)?,
                    value::OP_GE => self.ge_values(args)?,
                    value::OP_LT => self.lt_values(args)?,
                    value::OP_LE => self.le_values(args)?,
                    value::OP_POW => self.pow_values(args)?,
                    value::OP_COALESCE => self.coalesce_values(args)?,
                    value::OP_NEGATE => self.negate_values(args)?,
                    value::OP_MINUS => self.minus_values(args)?,
                    _ => { todo!() }
                })
            }
        }
    }

    fn coalesce_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let res = args.into_iter().try_fold(vec![], |mut accum, cur| {
            match self.partial_eval(cur) {
                Ok((ev, cur)) => {
                    if ev {
                        if cur == Value::Null {
                            Ok(accum)
                        } else {
                            Err(Ok(cur))
                        }
                    } else {
                        accum.push(cur);
                        Ok(accum)
                    }
                }
                Err(e) => Err(Err(e))
            }
        });
        match res {
            Ok(accum) => {
                match accum.len() {
                    0 => Ok((true, Value::Null)),
                    1 => Ok((false, accum.into_iter().next().unwrap())),
                    _ => Ok((false, Value::Apply(value::OP_COALESCE.into(), accum)))
                }
            }
            Err(Ok(v)) => Ok((true, v)),
            Err(Err(e)) => Err(e)
        }
    }

    fn add_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_ADD.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l + r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l + (r as f64)).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) + r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l.into_inner() + r.into_inner()).into()),
            (Value::Text(l), Value::Text(r)) => (true, (l.to_string() + r.as_ref()).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn sub_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_SUB.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l - r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l - (r as f64)).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) - r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l.into_inner() - r.into_inner()).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn minus_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le {
            return Ok((false, Value::Apply(value::OP_MINUS.into(), vec![left])));
        }
        Ok(match left {
            Value::Int(l) => (true, (-l).into()),
            Value::Float(l) => (true, (-l).into()),
            _ => return Err(CozoError::InvalidArgument)
        })
    }
    fn negate_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le {
            return Ok((false, Value::Apply(value::OP_NEGATE.into(), vec![left])));
        }
        Ok(match left {
            Value::Bool(l) => (true, (!l).into()),
            _ => return Err(CozoError::InvalidArgument)
        })
    }
    fn pow_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_POW.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, ((l as f64).powf(r as f64)).into()),
            (Value::Float(l), Value::Int(r)) => (true, ((l.into_inner()).powf(r as f64)).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64).powf(r.into_inner())).into()),
            (Value::Float(l), Value::Float(r)) => (true, ((l.into_inner()).powf(r.into_inner())).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn gt_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_GT.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l > r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l > (r as f64).into()).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) > r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l > r).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn lt_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_LT.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l < r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l < (r as f64).into()).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) < r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l < r).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn ge_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_GE.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l >= r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l >= (r as f64).into()).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) >= r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l >= r).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn le_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_GE.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l <= r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l <= (r as f64).into()).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) <= r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l <= r).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn mod_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_MOD.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l % r).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn mul_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_MUL.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l * r).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l * (r as f64)).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) * r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l.into_inner() * r.into_inner()).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn div_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_DIV.into(), vec![left, right])));
        }
        Ok(match (left, right) {
            (Value::Int(l), Value::Int(r)) => (true, (l as f64 / r as f64).into()),
            (Value::Float(l), Value::Int(r)) => (true, (l / (r as f64)).into()),
            (Value::Int(l), Value::Float(r)) => (true, ((l as f64) / r.into_inner()).into()),
            (Value::Float(l), Value::Float(r)) => (true, (l.into_inner() / r.into_inner()).into()),
            (_, _) => return Err(CozoError::InvalidArgument)
        })
    }
    fn eq_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_EQ.into(), vec![left, right])));
        }
        Ok((true, (left == right).into()))
    }
    fn ne_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let mut args = args.into_iter();
        let (le, left) = self.partial_eval(args.next().unwrap())?;
        let (re, right) = self.partial_eval(args.next().unwrap())?;
        if left == Value::Null || right == Value::Null {
            return Ok((true, Value::Null));
        }
        if !le || !re {
            return Ok((false, Value::Apply(value::OP_NE.into(), vec![left, right])));
        }
        Ok((true, (left != right).into()))
    }
    fn or_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let res = args.into_iter().map(|v| self.partial_eval(v))
            .try_fold(
                (true, false, vec![]),
                |(is_evaluated, has_null, mut collected), x| {
                    match x {
                        Ok((cur_eval, cur_val)) => {
                            if cur_eval {
                                match cur_val {
                                    Value::Null => {
                                        Ok((is_evaluated, true, collected))
                                    }
                                    Value::Bool(b) => if b {
                                        Err(Ok((true, Value::Bool(true)))) // Early return on true
                                    } else {
                                        Ok((is_evaluated, has_null, collected))
                                    },
                                    _ => Err(Err(CozoError::InvalidArgument))
                                }
                            } else {
                                match cur_val {
                                    Value::Null |
                                    Value::Bool(_) |
                                    Value::UInt(_) |
                                    Value::Int(_) |
                                    Value::Float(_) |
                                    Value::Uuid(_) |
                                    Value::EndSentinel |
                                    Value::Text(_) => unreachable!(),
                                    Value::List(_) |
                                    Value::Dict(_) => Err(Err(CozoError::InvalidArgument)),
                                    cur_val @ (Value::Variable(_) |
                                    Value::Apply(_, _)) => {
                                        collected.push(cur_val);
                                        Ok((false, has_null, collected))
                                    }
                                }
                            }
                        }
                        Err(e) => Err(Err(e))
                    }
                });
        match res {
            Ok((is_evaluated, has_null, mut unevaluated)) => {
                if is_evaluated {
                    if has_null {
                        Ok((true, Value::Null))
                    } else {
                        Ok((true, Value::Bool(false)))
                    }
                } else {
                    if has_null {
                        unevaluated.push(Value::Null);
                    }
                    Ok((false, Value::Apply(value::OP_OR.into(), unevaluated)))
                }
            }
            Err(Ok(res)) => Ok(res),
            Err(Err(e)) => Err(e)
        }
    }
    fn and_values<'a>(&self, args: Vec<Value<'a>>) -> Result<(bool, Value<'a>)> {
        let res = args.into_iter().map(|v| self.partial_eval(v))
            .try_fold(
                (true, false, vec![]),
                |(is_evaluated, has_null, mut collected), x| {
                    match x {
                        Ok((cur_eval, cur_val)) => {
                            if cur_eval {
                                match cur_val {
                                    Value::Null => {
                                        Ok((is_evaluated, true, collected))
                                    }
                                    Value::Bool(b) => if b {
                                        Ok((is_evaluated, has_null, collected))
                                    } else {
                                        Err(Ok((true, Value::Bool(false)))) // Early return on true
                                    },
                                    _ => Err(Err(CozoError::InvalidArgument))
                                }
                            } else {
                                match cur_val {
                                    Value::Null |
                                    Value::Bool(_) |
                                    Value::UInt(_) |
                                    Value::Int(_) |
                                    Value::Float(_) |
                                    Value::Uuid(_) |
                                    Value::EndSentinel |
                                    Value::Text(_) => unreachable!(),
                                    Value::List(_) |
                                    Value::Dict(_) => Err(Err(CozoError::InvalidArgument)),
                                    cur_val @ (Value::Variable(_) |
                                    Value::Apply(_, _)) => {
                                        collected.push(cur_val);
                                        Ok((false, has_null, collected))
                                    }
                                }
                            }
                        }
                        Err(e) => Err(Err(e))
                    }
                });
        match res {
            Ok((is_evaluated, has_null, mut unevaluated)) => {
                if is_evaluated {
                    if has_null {
                        Ok((true, Value::Null))
                    } else {
                        Ok((true, Value::Bool(true)))
                    }
                } else {
                    if has_null {
                        unevaluated.push(Value::Null);
                    }
                    Ok((false, Value::Apply(value::OP_AND.into(), unevaluated)))
                }
            }
            Err(Ok(res)) => Ok(res),
            Err(Err(e)) => Err(e)
        }
    }
}


pub struct MemoryEnv {
    root: BTreeMap<String, OwnTuple>,
    stack: Vec<BTreeMap<String, OwnTuple>>,
    max_storage_id: u32,
}

impl Default for MemoryEnv {
    fn default() -> Self {
        MemoryEnv { root: BTreeMap::default(), stack: vec![BTreeMap::default()], max_storage_id: 0 }
    }
}

impl Environment<Vec<u8>> for MemoryEnv {
    fn get_next_storage_id(&mut self, _in_root: bool) -> Result<u32> {
        self.max_storage_id += 1;
        Ok(self.max_storage_id)
    }

    fn get_stack_depth(&self) -> i32 {
        -(self.stack.len() as i32)
    }

    fn push_env(&mut self) {
        self.stack.push(BTreeMap::default());
    }

    fn pop_env(&mut self) -> Result<()> {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
        Ok(())
    }

    fn resolve(&self, name: &str) -> Result<Option<OwnTuple>> {
        for layer in self.stack.iter() {
            if let Some(res) = layer.get(name) {
                return Ok(Some(res.clone()));
            }
        }
        Ok(self.root.get(name).cloned())
    }

    fn delete_defined(&mut self, name: &str, in_root: bool) -> Result<()> {
        if in_root {
            self.root.remove(name);
        } else {
            for layer in self.stack.iter_mut().rev() {
                if let Some(_) = layer.remove(name) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn define_data(&mut self, name: &str, data: OwnTuple, in_root: bool) -> Result<()> {
        if in_root {
            self.root.insert(name.to_string(), data);
        } else {
            let last = self.stack.last_mut().unwrap();
            last.insert(name.to_string(), data);
        }
        Ok(())
    }
}


impl<'a> Environment<SlicePtr> for Session<'a> {
    fn get_next_storage_id(&mut self, in_root: bool) -> Result<u32> {
        // TODO: deal with wrapping problem
        let mut key_entry = Tuple::with_null_prefix();
        key_entry.push_null();
        let db_res = if in_root {
            self.txn.get(true, &self.perm_cf, &key_entry)
        } else {
            self.txn.get(false, &self.temp_cf, &key_entry)
        };
        let u = if let Some(en) = db_res? {
            if let Value::UInt(u) = Tuple::new(en).get(0).unwrap() {
                u
            } else {
                panic!("Unexpected value in storage id");
            }
        } else { 0 };
        let mut new_data = Tuple::with_null_prefix();
        new_data.push_uint(u + 1);
        if in_root {
            self.txn.put(true, &self.perm_cf, key_entry, new_data)?;
        } else {
            self.txn.put(false, &self.temp_cf, key_entry, new_data)?;
        }
        Ok((u + 1) as u32)
    }

    fn get_stack_depth(&self) -> i32 {
        self.stack_depth
    }

    fn push_env(&mut self) {
        self.stack_depth -= 1;
    }

    fn pop_env(&mut self) -> Result<()> {
        // Remove all stuff starting with the stack depth from the temp session
        let mut prefix = Tuple::with_null_prefix();
        prefix.push_int(self.stack_depth as i64);
        let it = self.txn.iterator(false, &self.temp_cf);
        it.seek(&prefix);
        for val in it.keys() {
            let cur = Tuple::new(val);
            if cur.starts_with(&prefix) {
                if let Some(name) = cur.get(1) {
                    let mut ikey = Tuple::with_null_prefix();
                    ikey.push_value(&name);
                    ikey.push_int(self.stack_depth as i64);

                    self.txn.del(false, &self.temp_cf, cur)?;
                    self.txn.del(false, &self.temp_cf, ikey)?;
                }
            } else {
                break;
            }
        }

        if self.stack_depth != 0 {
            self.stack_depth += 1;
        }
        Ok(())
    }

    fn resolve(&self, name: &str) -> Result<Option<Tuple<SlicePtr>>> {
        let mut tuple = Tuple::with_null_prefix();
        tuple.push_str(name);
        let it = self.txn.iterator(false, &self.temp_cf);
        it.seek(&tuple);
        if let Some((tk, vk)) = it.pair() {
            let k = Tuple::new(tk);
            if k.starts_with(&tuple) {
                return Ok(Some(Tuple::new(vk)));
            }
        }
        let root_key = self.encode_definable_key(name, true);
        let res = self.txn.get(true, &self.perm_cf, root_key).map(|v| v.map(Tuple::new))?;
        Ok(res)
    }

    fn delete_defined(&mut self, name: &str, in_root: bool) -> Result<()> {
        let key = self.encode_definable_key(name, in_root);
        if in_root {
            self.txn.del(true, &self.perm_cf, key)?;
        } else {
            let it = self.txn.iterator(false, &self.temp_cf);
            it.seek(&key);
            if let Some(found_key) = it.key() {
                let found_key_tuple = Tuple::new(found_key);
                if found_key_tuple.starts_with(&key) {
                    let mut ikey = Tuple::with_null_prefix();
                    ikey.push_value(&found_key_tuple.get(1).unwrap());
                    ikey.push_value(&found_key_tuple.get(0).unwrap());
                    self.txn.del(false, &self.temp_cf, found_key_tuple)?;
                    self.txn.del(false, &self.temp_cf, ikey)?;
                }
            }
        }
        // TODO cleanup if the thing deleted is a table

        Ok(())
    }

    fn define_data(&mut self, name: &str, data: OwnTuple, in_root: bool) -> Result<()> {
        let key = self.encode_definable_key(name, in_root);
        if in_root {
            self.txn.put(true, &self.perm_cf, key, data)?;
        } else {
            let mut ikey = Tuple::with_null_prefix();
            ikey.push_int(self.stack_depth as i64);
            ikey.push_str(name);
            self.txn.put(false, &self.temp_cf, key, data)?;
            self.txn.put(false, &self.temp_cf, ikey, "")?;
        }
        Ok(())
    }
}


impl<'a> Session<'a> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Parser, Rule};
    use pest::Parser as PestParser;
    use crate::db::eval::MemoryEnv;

    #[test]
    fn node() {
        let s = r#"
            create node "Person" {
                *id: Int,
                name: Text,
                email: ?Text,
                habits: ?[?Text]
            }

            create edge (Person)-[Friend]->(Person) {
                relation: ?Text
            }

            create type XXY = {me: Int, f: ?[Text]}

            local assoc WorkInfo: Person {
                email: Text
            }
        "#;
        let mut env = MemoryEnv::default();
        let mut parsed = Parser::parse(Rule::file, s).unwrap();

        let t = parsed.next().unwrap();
        env.run_definition(t).unwrap();
        println!("{:?}", env.resolve("Person"));

        let t = parsed.next().unwrap();
        env.run_definition(t).unwrap();
        println!("{:?}", env.resolve("Friend"));

        let t = parsed.next().unwrap();
        env.run_definition(t).unwrap();
        println!("{:?}", env.resolve("XXY"));

        let t = parsed.next().unwrap();
        env.run_definition(t).unwrap();
        println!("{:?}", env.resolve("WorkInfo"));
    }

    fn parse_expr_from_str(s: &str) -> (bool, Value) {
        MemoryEnv::default().partial_eval(Value::from_pair(Parser::parse(Rule::expr, s).unwrap().next().unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn eval_expr() {
        assert_eq!((true, Value::from(1024.1)), parse_expr_from_str("1/10+(-2+3)*4^5"));
        assert_eq!((true, Value::from(false)), parse_expr_from_str("true && false"));
        assert_eq!((true, Value::from(true)), parse_expr_from_str("true || false"));
        assert_eq!((true, Value::from(true)), parse_expr_from_str("true || null"));
        assert_eq!((true, Value::from(true)), parse_expr_from_str("null || true"));
        assert_eq!((true, Value::Null), parse_expr_from_str("true && null"));
        let ex = parse_expr_from_str("a + b - 1*2*3*100*c * d");
        println!("{:?} {}", ex.0, ex.1);
    }
}

//     fn test_null_expr<'a>(&self, exprs: &[Expr<'a>]) -> Result<Expr<'a>> {
//         Ok(match exprs {
//             [a] => {
//                 match self.visit_expr(a)? {
//                     Const(Null) => Const(Bool(true)),
//                     Const(_) => Const(Bool(false)),
//                     v => Value::Apply(Op::IsNull, vec![v])
//                 }
//             }
//             _ => unreachable!()
//         })
//     }
//
//     fn not_null_expr<'a>(&self, exprs: &[Expr<'a>]) -> Result<Expr<'a>> {
//         Ok(match exprs {
//             [a] => {
//                 match self.visit_expr(a)? {
//                     Const(Null) => Const(Bool(false)),
//                     Const(_) => Const(Bool(true)),
//                     v => Value::Apply(Op::IsNull, vec![v])
//                 }
//             }
//             _ => unreachable!()
//         })
//     }
// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn operators() {
//         let ev = Evaluator::new(DummyStorage {}).unwrap();
//
//     }
// }