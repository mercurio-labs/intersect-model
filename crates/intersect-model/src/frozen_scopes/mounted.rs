//! The same verified direct programs over an immutable mounted read index.
//! The same authoritative VM handles cases outside the direct subset.
use super::*;
use crate::rcore_fast::{FastMemo, FastRead};

impl RcoreScopes<'_> {
    pub(crate) fn check_mounted_schema(&self, metamodel: &Metamodel) -> Result<(), String> {
        if self.mounted_schema_digest != metamodel.digest().map_err(|e| e.to_string())? {
            return Err("Scope bindings belong to another mounted metamodel".into());
        }
        Ok(())
    }
    fn mounted_run(
        &self,
        route: &Route<'_>,
        read: &dyn FastRead,
        memo: &FastMemo,
        owner: ObjId,
        spelling: Option<&str>,
    ) -> Result<V, FastError> {
        let program = route.fast.as_ref().ok_or(FastError::Fallback(
            route.unsupported.unwrap_or("binding has no direct program"),
        ))?;
        let arguments = match spelling {
            Some(spelling) => vec![V::Obj(owner), V::Str(Rc::from(spelling))],
            None => vec![V::Obj(owner)],
        };
        self.fast.set(self.fast.get() + 1);
        program.run_memo(read, arguments, self.fast_limits(), memo)
    }

    pub(crate) fn mounted_candidates(
        &self,
        read: &dyn IndexedScopeRead,
        memo: &FastMemo,
        owner: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<Option<Vec<ObjId>>, String> {
        let vm = || {
            let route = self
                .routes
                .get(&feature)
                .ok_or("No source scope is bound to this member")?;
            let owner_id = read.identity(owner).ok_or("Missing scope owner")?;
            let adapter = StoredScopeRead::new(read);
            let mut caller = QueryBudget::new(self.budget);
            let work = BorrowedBindingWork::bind(&mut caller);
            self.vm.set(self.vm.get() + 1);
            let found = match scope_runtime::evaluate(
                route.scope,
                &ExecutionContext::new(self.bundle),
                read.metamodel().schema(),
                &adapter,
                owner_id,
                spelling,
                &self.limits,
                &work,
            ) {
                Ok(found) => found,
                Err(
                    scope_runtime::ScopeError::Pending { .. }
                    | scope_runtime::ScopeError::PendingOwnership { .. },
                ) => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
            found
                .identities()
                .iter()
                .map(|id| {
                    read.lookup(id)
                        .ok_or_else(|| "Missing VM scope candidate".into())
                })
                .collect::<Result<Vec<_>, String>>()
                .map(Some)
        };
        if matches!(self.execution, Execution::Vm) {
            return vm();
        }
        match self.mounted_candidates_fast(read, memo, owner, feature, spelling) {
            Ok(found) => {
                if matches!(self.execution, Execution::Shadow) && vm()?.as_ref() != Some(&found) {
                    return Err("Mounted direct scope disagrees with the VM".into());
                }
                Ok(Some(found))
            }
            Err(FastError::Pending) => Ok(None),
            Err(FastError::Fallback(reason)) => {
                self.deferred.set(self.deferred.get() + 1);
                *self.reasons.borrow_mut().entry(reason).or_insert(0) += 1;
                vm()
            }
        }
    }

    fn mounted_candidates_fast(
        &self,
        read: &dyn FastRead,
        memo: &FastMemo,
        owner: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<Vec<ObjId>, FastError> {
        let route = self.routes.get(&feature).ok_or(FastError::Fallback(
            "no source scope is bound to this member",
        ))?;
        let V::Seq(items) = self.mounted_run(route, read, memo, owner, Some(spelling))? else {
            return Err(FastError::Fallback("scope result is not a sequence"));
        };
        let target = route
            .target
            .ok_or(FastError::Fallback("scope target is not a model class"))?;
        let mut found = Vec::new();
        for item in items.iter() {
            let V::Obj(object) = item else {
                return Err(FastError::Fallback("scope candidate is not a model object"));
            };
            let class = read
                .class(*object)
                .ok_or(FastError::Fallback("scope candidate has no class"))?;
            if !read.metamodel().is_a(class, target) {
                return Err(FastError::Fallback(
                    "scope candidate does not conform to its target",
                ));
            }
            if !found.contains(object) {
                found.push(*object);
            }
        }
        Ok(found)
    }

    pub(crate) fn mounted_implicit_features(&self) -> Vec<FeatureId> {
        self.implicits.keys().copied().collect()
    }
    pub(crate) fn mounted_default_features(&self) -> Vec<FeatureId> {
        self.defaults.keys().copied().collect()
    }

    pub(crate) fn mounted_implicits(
        &self,
        read: &dyn FastRead,
        memo: &FastMemo,
        owner: ObjId,
        feature: FeatureId,
    ) -> Result<Vec<ImplicitObject>, String> {
        let class = read.class(owner).ok_or("Missing implicit receiver")?;
        let mut result = Vec::new();
        for (receiver, route) in self
            .implicits
            .get(&feature)
            .ok_or("Missing implicit binding")?
        {
            if !read.metamodel().is_a(class, *receiver) {
                continue;
            }
            let V::Seq(items) = self
                .mounted_run(route, read, memo, owner, None)
                .map_err(|e| format!("Mounted implicit binding: {e:?}"))?
            else {
                return Err("Implicit binding result is not a sequence".into());
            };
            for item in items.iter() {
                result.push(implicit_object(item).map_err(|e| e.to_string())?);
            }
        }
        Ok(result)
    }

    pub(crate) fn mounted_defaults(
        &self,
        read: &dyn FastRead,
        memo: &FastMemo,
        owner: ObjId,
        feature: FeatureId,
    ) -> Result<Vec<DefaultItem>, String> {
        let route = self
            .defaults
            .get(&feature)
            .ok_or("Missing default binding")?;
        let V::Seq(items) = self
            .mounted_run(route, read, memo, owner, None)
            .map_err(|e| format!("Mounted default binding: {e:?}"))?
        else {
            return Err("Default binding result is not a sequence".into());
        };
        items
            .iter()
            .map(|item| match item {
                V::Obj(object) if route.scope.binding().attribute.is_none() => {
                    Ok(DefaultItem::Object(*object))
                }
                V::Str(v) => Ok(DefaultItem::Value(Value::String(v.to_string()))),
                V::Bool(v) => Ok(DefaultItem::Value(Value::Bool(*v))),
                V::Int(v) => Ok(DefaultItem::Value(Value::from(*v))),
                V::Enum(v) => Ok(DefaultItem::Value(Value::clone(v))),
                _ => Err("Unsupported value returned by a mounted default binding".into()),
            })
            .collect()
    }

    pub(crate) fn mounted_issues(
        &self,
        read: &dyn IndexedScopeRead,
        memo: &FastMemo,
        owner: ObjId,
    ) -> Result<Vec<Issue>, String> {
        let class = read.class(owner).ok_or("Missing validation receiver")?;
        let mut result = Vec::new();
        for (receiver, route) in &self.validators {
            if !read.metamodel().is_a(class, *receiver) {
                continue;
            }
            let vm = || {
                let owner_id = read.identity(owner).ok_or("Missing validation owner")?;
                let adapter = StoredScopeRead::new(read);
                let mut caller = QueryBudget::new(self.budget);
                let work = BorrowedBindingWork::bind(&mut caller);
                self.vm.set(self.vm.get() + 1);
                let value = scope_runtime::evaluate_read_function(
                    route.scope.executable(),
                    &ExecutionContext::new(self.bundle),
                    read.metamodel().schema(),
                    &adapter,
                    owner_id,
                    Vec::new(),
                    &self.limits,
                    &work,
                )
                .map_err(|e| e.to_string())?;
                vm_issues(&value.data).map_err(|e| e.to_string())
            };
            let found = if matches!(self.execution, Execution::Vm) {
                vm()?
            } else {
                match self
                    .mounted_run(route, read, memo, owner, None)
                    .and_then(|value| fast_issues(&value))
                {
                    Ok(found) => {
                        if matches!(self.execution, Execution::Shadow) && vm()? != found {
                            return Err("Mounted direct validation disagrees with the VM".into());
                        }
                        found
                    }
                    Err(FastError::Pending) => {
                        return Err("Mounted validation read a pending reference".into());
                    }
                    Err(FastError::Fallback(reason)) => {
                        self.deferred.set(self.deferred.get() + 1);
                        *self.reasons.borrow_mut().entry(reason).or_insert(0) += 1;
                        vm()?
                    }
                }
            };
            result.extend(found);
        }
        Ok(result)
    }
}
