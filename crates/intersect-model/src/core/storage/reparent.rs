//! Explicit stored containment transfers, including absent and cross-feature inverses.
use super::*;
use crate::core::query::QueryBudget;

#[derive(Clone, Copy)]
pub struct ContainmentDestination<'a> {
    pub owner: &'a str,
    pub feature: &'a str,
    /// Final index for an ordered Many feature. None preserves an existing
    /// position or appends a newly transferred object.
    pub position: Option<usize>,
}
#[derive(Clone, Copy)]
pub struct ReparentRequest<'a> {
    pub object: &'a str,
    /// None detaches to a root while retaining the object's source/resource identity.
    pub destination: Option<ContainmentDestination<'a>>,
}
fn members(
    store: &impl ModelRead,
    owner: &str,
    name: &str,
    budget: &mut QueryBudget,
) -> StorageResult<Vec<Value>> {
    budget.charge()?;
    let mut result = Vec::new();
    if !store.inspect(owner, &mut |object| {
        let value = object.value(name).ok_or("Missing containment value")?;
        let mut push = |value: ValueView<'_>| -> StorageResult<()> {
            budget.charge()?;
            result.push(Value::String(
                value
                    .as_str()
                    .ok_or("Invalid containment identity")?
                    .to_owned(),
            ));
            Ok(())
        };
        if let Some(values) = value.collection() {
            for value in values.iter() {
                push(value)?;
            }
        } else if !value.is_null() {
            push(value)?;
        }
        Ok(())
    })? {
        return Err("Missing containment owner".into());
    }
    Ok(result)
}
impl StructuralContext<'_> {
    /// Transfer or detach one stored object atomically. Singular destination
    /// displacement detaches its previous child without deleting that subtree.
    /// Observed validation, planning and writes consume the supplied budget.
    pub fn reparent(
        &self,
        store: &mut impl ModelStore,
        request: ReparentRequest<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<bool> {
        budget.charge()?;
        self.validate(store, budget)?;
        let object_class = store
            .class(request.object)?
            .ok_or("Missing reparent target")?;
        let mut old = None;
        let mut cursor = None;
        loop {
            budget.charge()?;
            let rows = store.id_page(cursor, 128)?;
            if rows.is_empty() {
                break;
            }
            for (row, id) in rows {
                cursor = Some(row);
                budget.charge()?;
                let class = store.class(&id)?.ok_or("Missing containment owner")?;
                for (name, field) in self.schema.features(&class)? {
                    budget.charge()?;
                    if !field.containment || field.derived {
                        continue;
                    }
                    let values = members(store, &id, &name, budget)?;
                    if values
                        .iter()
                        .any(|value| value.as_str() == Some(request.object))
                    {
                        if old.is_some() {
                            return Err("Multiple containment owners".into());
                        }
                        old = Some((id.clone(), name, field, values));
                    }
                }
            }
        }
        let mut target = None;
        if let Some(destination) = request.destination {
            budget.charge()?;
            let class = store
                .class(destination.owner)?
                .ok_or("Missing destination owner")?;
            let fields = self.schema.features(&class)?;
            let field = fields
                .get(destination.feature)
                .ok_or("Unknown containment feature")?;
            if !field.reference || !field.containment || field.derived {
                return Err("Reparent requires a stored containment feature".into());
            }
            if !field.changeable {
                return Err("Cannot reparent into a non-changeable feature".into());
            }
            if !self
                .schema
                .reference_conforms(&class, destination.feature, &object_class)?
            {
                return Err("Reparent target has an incompatible class".into());
            }
            if destination.position.is_some() && (field.upper == 1 || !field.ordered) {
                return Err("Containment positions require an ordered Many feature".into());
            }
            let same = old.as_ref().is_some_and(|(owner, name, _, _)| {
                owner == destination.owner && name == destination.feature
            });
            if same && destination.position.is_none() {
                return Ok(false);
            }
            let original = members(store, destination.owner, destination.feature, budget)?;
            let mut values = original.clone();
            if field.upper == 1 {
                values = vec![Value::String(request.object.into())];
            } else {
                if same {
                    values.retain(|v| v.as_str() != Some(request.object));
                }
                let position = destination.position.unwrap_or(values.len());
                if position > values.len() {
                    return Err("Containment position is out of bounds".into());
                }
                values.insert(position, Value::String(request.object.into()));
                if field.upper >= 0 && values.len() > field.upper as usize {
                    return Err("Containment destination exceeds its upper bound".into());
                }
            }
            if same && values == original {
                return Ok(false);
            }
            target = Some((destination, field.upper != 1, values, same));
        } else if old.is_none() {
            return Ok(false);
        }
        if let Some((_, _, field, _)) = &old
            && !field.changeable
        {
            return Err("Cannot detach a non-changeable containment feature".into());
        }
        atomic(store, |store| {
            bind_schema(store, self.schema)?;
            if !target.as_ref().is_some_and(|(_, _, _, same)| *same)
                && let Some((owner, name, field, mut values)) = old
            {
                budget.charge()?;
                if field.upper == 1 {
                    match self.conversions {
                        Some(registry) => {
                            unset_with_conversions(store, self.schema, &owner, &name, registry)?
                        }
                        None => unset(store, self.schema, &owner, &name)?,
                    }
                } else {
                    values.retain(|value| value.as_str() != Some(request.object));
                    set_inner(
                        store,
                        self.schema,
                        &owner,
                        &name,
                        Value::Array(values),
                        self.conversions,
                    )?;
                }
            }
            if let Some((destination, many, mut values, _)) = target {
                budget.charge()?;
                let value = if many {
                    Value::Array(values)
                } else {
                    values.pop().ok_or("Missing singular reparent value")?
                };
                set_inner(
                    store,
                    self.schema,
                    destination.owner,
                    destination.feature,
                    value,
                    self.conversions,
                )?;
            }
            self.validate(store, budget)?;
            Ok(true)
        })
    }
}
