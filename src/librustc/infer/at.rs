// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A nice interface for working with the infcx.  The basic idea is to
//! do `infcx.at(cause, param_env)`, which sets the "cause" of the
//! operation as well as the surrounding parameter environment.  Then
//! you can do something like `.sub(a, b)` or `.eq(a, b)` to create a
//! subtype or equality relationship respectively. The first argument
//! is always the "expected" output from the POV of diagnostics.
//!
//! Examples:
//!
//!     infcx.at(cause, param_env).sub(a, b)
//!     // requires that `a <: b`, with `a` considered the "expected" type
//!
//!     infcx.at(cause, param_env).sup(a, b)
//!     // requires that `b <: a`, with `a` considered the "expected" type
//!
//!     infcx.at(cause, param_env).eq(a, b)
//!     // requires that `a == b`, with `a` considered the "expected" type
//!
//! For finer-grained control, you can also do use `trace`:
//!
//!     infcx.at(...).trace(a, b).sub(&c, &d)
//!
//! This will set `a` and `b` as the "root" values for
//! error-reporting, but actually operate on `c` and `d`. This is
//! sometimes useful when the types of `c` and `d` are not traceable
//! things. (That system should probably be refactored.)

use super::*;

use ty::relate::{Relate, TypeRelation};

pub struct At<'a, 'gcx: 'tcx, 'tcx: 'a> {
    infcx: &'a InferCtxt<'a, 'gcx, 'tcx>,
    cause: &'a ObligationCause<'tcx>,
    param_env: ty::ParamEnv<'tcx>,
}

pub struct Trace<'a, 'gcx: 'tcx, 'tcx: 'a> {
    at: At<'a, 'gcx, 'tcx>,
    a_is_expected: bool,
    trace: TypeTrace<'tcx>,
}

impl<'a, 'gcx, 'tcx> InferCtxt<'a, 'gcx, 'tcx> {
    pub fn at(&'a self,
              cause: &'a ObligationCause<'tcx>,
              param_env: ty::ParamEnv<'tcx>)
              -> At<'a, 'gcx, 'tcx>
    {
        At { infcx: self, cause, param_env }
    }
}

pub trait ToTrace<'tcx>: Relate<'tcx> + Copy {
    fn to_trace(cause: &ObligationCause<'tcx>,
                a_is_expected: bool,
                a: Self,
                b: Self)
                -> TypeTrace<'tcx>;
}

impl<'a, 'gcx, 'tcx> At<'a, 'gcx, 'tcx> {
    /// Hacky routine for equating two impl headers in coherence.
    pub fn eq_impl_headers(self,
                           expected: &ty::ImplHeader<'tcx>,
                           actual: &ty::ImplHeader<'tcx>)
                           -> InferResult<'tcx, ()>
    {
        debug!("eq_impl_header({:?} = {:?})", expected, actual);
        match (expected.trait_ref, actual.trait_ref) {
            (Some(a_ref), Some(b_ref)) =>
                self.eq(a_ref, b_ref),
            (None, None) =>
                self.eq(expected.self_ty, actual.self_ty),
            _ =>
                bug!("mk_eq_impl_headers given mismatched impl kinds"),
        }
    }

    /// Make `a <: b` where `a` may or may not be expected
    pub fn sub_exp<T>(self,
                      a_is_expected: bool,
                      a: T,
                      b: T)
                      -> InferResult<'tcx, ()>
        where T: ToTrace<'tcx>
    {
        self.trace_exp(a_is_expected, a, b).sub(&a, &b)
    }

    /// Make `actual <: expected`. For example, if type-checking a
    /// call like `foo(x)`, where `foo: fn(i32)`, you might have
    /// `sup(i32, x)`, since the "expected" type is the type that
    /// appears in the signature.
    pub fn sup<T>(self,
                  expected: T,
                  actual: T)
                  -> InferResult<'tcx, ()>
        where T: ToTrace<'tcx>
    {
        self.sub_exp(false, actual, expected)
    }

    /// Make `expected <: actual`
    pub fn sub<T>(self,
                  expected: T,
                  actual: T)
                  -> InferResult<'tcx, ()>
        where T: ToTrace<'tcx>
    {
        self.sub_exp(true, expected, actual)
    }

    /// Make `expected <: actual`
    pub fn eq_exp<T>(self,
                     a_is_expected: bool,
                     a: T,
                     b: T)
                     -> InferResult<'tcx, ()>
        where T: ToTrace<'tcx>
    {
        self.trace_exp(a_is_expected, a, b).eq(&a, &b)
    }

    /// Make `expected <: actual`
    pub fn eq<T>(self,
                 expected: T,
                 actual: T)
                 -> InferResult<'tcx, ()>
        where T: ToTrace<'tcx>
    {
        self.trace(expected, actual).eq(&expected, &actual)
    }

    /// Compute the least-upper-bound, or mutual supertype, of two
    /// values. The order of the arguments doesn't matter, but since
    /// this can result in an error (e.g., if asked to compute LUB of
    /// u32 and i32), it is meaningful to call one of them the
    /// "expected type".
    pub fn lub<T>(self,
                  expected: T,
                  actual: T)
                  -> InferResult<'tcx, T>
        where T: ToTrace<'tcx>
    {
        self.trace(expected, actual).lub(&expected, &actual)
    }

    /// Compute the greatest-lower-bound, or mutual subtype, of two
    /// values. As with `lub` order doesn't matter, except for error
    /// cases.
    pub fn glb<T>(self,
                  expected: T,
                  actual: T)
                  -> InferResult<'tcx, T>
        where T: ToTrace<'tcx>
    {
        self.trace(expected, actual).glb(&expected, &actual)
    }

    /// Sets the "trace" values that will be used for
    /// error-repporting, but doesn't actually perform any operation
    /// yet (this is useful when you want to set the trace using
    /// distinct values from those you wish to operate upon).
    pub fn trace<T>(self,
                    expected: T,
                    actual: T)
                    -> Trace<'a, 'gcx, 'tcx>
        where T: ToTrace<'tcx>
    {
        self.trace_exp(true, expected, actual)
    }

    /// Like `trace`, but the expected value is determined by the
    /// boolean argument (if true, then the first argument `a` is the
    /// "expected" value).
    pub fn trace_exp<T>(self,
                        a_is_expected: bool,
                        a: T,
                        b: T)
                        -> Trace<'a, 'gcx, 'tcx>
        where T: ToTrace<'tcx>
    {
        let trace = ToTrace::to_trace(self.cause, a_is_expected, a, b);
        Trace { at: self, trace: trace, a_is_expected }
    }
}

impl<'a, 'gcx, 'tcx> Trace<'a, 'gcx, 'tcx> {
    /// Make `a <: b` where `a` may or may not be expected (if
    /// `a_is_expected` is true, then `a` is expected).
    /// Make `expected <: actual`
    pub fn sub<T>(self,
                  a: &T,
                  b: &T)
                  -> InferResult<'tcx, ()>
        where T: Relate<'tcx>
    {
        debug!("sub({:?} <: {:?})", a, b);
        let Trace { at, trace, a_is_expected } = self;
        at.infcx.commit_if_ok(|_| {
            let mut fields = at.infcx.combine_fields(trace, at.param_env);
            fields.sub(a_is_expected)
                  .relate(a, b)
                  .map(move |_| InferOk { value: (), obligations: fields.obligations })
        })
    }

    /// Make `a == b`; the expectation is set by the call to
    /// `trace()`.
    pub fn eq<T>(self,
                 a: &T,
                 b: &T)
                 -> InferResult<'tcx, ()>
        where T: Relate<'tcx>
    {
        debug!("eq({:?} == {:?})", a, b);
        let Trace { at, trace, a_is_expected } = self;
        at.infcx.commit_if_ok(|_| {
            let mut fields = at.infcx.combine_fields(trace, at.param_env);
            fields.equate(a_is_expected)
                  .relate(a, b)
                  .map(move |_| InferOk { value: (), obligations: fields.obligations })
        })
    }

    pub fn lub<T>(self,
                  a: &T,
                  b: &T)
                  -> InferResult<'tcx, T>
        where T: Relate<'tcx>
    {
        debug!("lub({:?} \\/ {:?})", a, b);
        let Trace { at, trace, a_is_expected } = self;
        at.infcx.commit_if_ok(|_| {
            let mut fields = at.infcx.combine_fields(trace, at.param_env);
            fields.lub(a_is_expected)
                  .relate(a, b)
                  .map(move |t| InferOk { value: t, obligations: fields.obligations })
        })
    }

    pub fn glb<T>(self,
                  a: &T,
                  b: &T)
                  -> InferResult<'tcx, T>
        where T: Relate<'tcx>
    {
        debug!("glb({:?} /\\ {:?})", a, b);
        let Trace { at, trace, a_is_expected } = self;
        at.infcx.commit_if_ok(|_| {
            let mut fields = at.infcx.combine_fields(trace, at.param_env);
            fields.glb(a_is_expected)
                  .relate(a, b)
                  .map(move |t| InferOk { value: t, obligations: fields.obligations })
        })
    }
}

impl<'tcx> ToTrace<'tcx> for Ty<'tcx> {
    fn to_trace(cause: &ObligationCause<'tcx>,
                a_is_expected: bool,
                a: Self,
                b: Self)
                -> TypeTrace<'tcx>
    {
        TypeTrace {
            cause: cause.clone(),
            values: Types(ExpectedFound::new(a_is_expected, a, b))
        }
    }
}

impl<'tcx> ToTrace<'tcx> for ty::TraitRef<'tcx> {
    fn to_trace(cause: &ObligationCause<'tcx>,
                a_is_expected: bool,
                a: Self,
                b: Self)
                -> TypeTrace<'tcx>
    {
        TypeTrace {
            cause: cause.clone(),
            values: TraitRefs(ExpectedFound::new(a_is_expected, a, b))
        }
    }
}

impl<'tcx> ToTrace<'tcx> for ty::PolyTraitRef<'tcx> {
    fn to_trace(cause: &ObligationCause<'tcx>,
                a_is_expected: bool,
                a: Self,
                b: Self)
                -> TypeTrace<'tcx>
    {
        TypeTrace {
            cause: cause.clone(),
            values: PolyTraitRefs(ExpectedFound::new(a_is_expected, a, b))
        }
    }
}
