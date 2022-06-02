use crate::data::eval::EvalError;
use crate::data::expr::Expr;
use crate::data::op_agg::{OpAgg, OpAggT};
use crate::data::value::{StaticValue, Value};
use anyhow::Result;
use ordered_float::Float;
use std::sync::{Arc, Mutex};

pub(crate) const NAME_OP_MIN: &str = "min";
pub(crate) const NAME_OP_MAX: &str = "max";

pub(crate) fn build_op_min(a_args: Vec<Expr>, args: Vec<Expr>) -> Expr {
    Expr::ApplyAgg(OpAgg(Arc::new(OpMin::default())), a_args, args)
}

pub(crate) fn build_op_max(a_args: Vec<Expr>, args: Vec<Expr>) -> Expr {
    Expr::ApplyAgg(OpAgg(Arc::new(OpMax::default())), a_args, args)
}

#[derive(Default)]
pub struct OpMin {
    total: Mutex<f64>,
}

impl OpAggT for OpMin {
    fn name(&self) -> &str {
        NAME_OP_MIN
    }

    fn arity(&self) -> Option<usize> {
        Some(1)
    }

    fn reset(&self) {
        let mut total = self.total.lock().unwrap();
        *total = f64::max_value();
    }

    fn initialize(&self, _a_args: Vec<StaticValue>) -> Result<()> {
        Ok(())
    }

    fn put(&self, args: &[Value]) -> Result<()> {
        let arg = args.iter().next().unwrap();
        let to_add = match arg {
            Value::Int(i) => (*i) as f64,
            Value::Float(f) => f.into_inner(),
            Value::Null => return Ok(()),
            v => {
                return Err(EvalError::OpTypeMismatch(
                    self.name().to_string(),
                    vec![v.clone().into_static()],
                )
                .into())
            }
        };
        let current = *self.total.lock().unwrap();
        *self.total.lock().unwrap() = current.min(to_add);
        Ok(())
    }

    fn get(&self) -> Result<StaticValue> {
        let f = *self.total.lock().unwrap();
        Ok(f.into())
    }
}

#[derive(Default)]
pub struct OpMax {
    total: Mutex<f64>,
}

impl OpAggT for OpMax {
    fn name(&self) -> &str {
        NAME_OP_MAX
    }

    fn arity(&self) -> Option<usize> {
        Some(1)
    }

    fn reset(&self) {
        let mut total = self.total.lock().unwrap();
        *total = f64::min_value();
    }

    fn initialize(&self, _a_args: Vec<StaticValue>) -> Result<()> {
        Ok(())
    }

    fn put(&self, args: &[Value]) -> Result<()> {
        let arg = args.iter().next().unwrap();
        let to_add = match arg {
            Value::Int(i) => (*i) as f64,
            Value::Float(f) => f.into_inner(),
            Value::Null => return Ok(()),
            v => {
                return Err(EvalError::OpTypeMismatch(
                    self.name().to_string(),
                    vec![v.clone().into_static()],
                )
                .into())
            }
        };
        let current = *self.total.lock().unwrap();
        *self.total.lock().unwrap() = current.max(to_add);
        Ok(())
    }

    fn get(&self) -> Result<StaticValue> {
        let f = *self.total.lock().unwrap();
        Ok(f.into())
    }
}
