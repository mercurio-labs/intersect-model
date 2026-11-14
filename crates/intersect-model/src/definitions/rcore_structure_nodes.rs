//! Native declaration construction: no generated XML or independent semantic rules.
use super::*;
use std::collections::BTreeMap;
type Scope = BTreeMap<String, String>;

struct Context<'a> {
    names: &'a BTreeSet<String>,
    package: String,
    scope: Scope,
    traversal: &'a preparation::Traversal,
}
impl Context<'_> {
    fn reference(
        &self,
        name: &str,
        span: &SourceSpan,
        quoted: bool,
    ) -> Result<String, ImportError> {
        if let Some(builtin) = scalar(name).filter(|_| !quoted) {
            return Ok(format!("{ECORE}#//{builtin}"));
        }
        let local = format!("{}{name}", self.package);
        if self.names.contains(&local) {
            Ok(format!("#//{local}"))
        } else if self.names.contains(name) {
            Ok(format!("#//{name}"))
        } else {
            Err(failure(span, format!("Unknown structural type {name}")))
        }
    }
    async fn generic(&self, name: &str, ty: &TypeSyntax) -> Result<Node, ImportError> {
        let traversal = self.traversal;
        let mut result = node(name, None, &ty.span, traversal).await;
        match &ty.kind {
            TypeSyntaxKind::Explicit(inner) => return Box::pin(self.generic(name, inner)).await,
            TypeSyntaxKind::Named(name, arguments, quoted) => {
                if let Some(parameter) = self.scope.get(name) {
                    if !arguments.is_empty() {
                        return Err(failure(&ty.span, "A type parameter cannot take arguments"));
                    }
                    attribute(&mut result, "eTypeParameter", parameter, traversal).await;
                } else {
                    attribute(
                        &mut result,
                        "eClassifier",
                        self.reference(name, &ty.span, *quoted)?,
                        traversal,
                    )
                    .await;
                    for argument in arguments {
                        {
                            let child = Box::pin(self.generic("eTypeArguments", argument)).await?;
                            result.children.push(child);
                            traversal.tree().await;
                        };
                    }
                }
            }
            TypeSyntaxKind::External(href, arguments) => {
                attribute(&mut result, "eClassifier", href, traversal).await;
                for argument in arguments {
                    {
                        let child = Box::pin(self.generic("eTypeArguments", argument)).await?;
                        result.children.push(child);
                        traversal.tree().await;
                    };
                }
            }
            TypeSyntaxKind::Parameter(href) => {
                attribute(&mut result, "eTypeParameter", href, traversal).await
            }
            TypeSyntaxKind::Wildcard { upper, lower } => {
                if let Some(upper) = upper {
                    {
                        let child = Box::pin(self.generic("eUpperBound", upper)).await?;
                        result.children.push(child);
                        traversal.tree().await;
                    };
                }
                if let Some(lower) = lower {
                    {
                        let child = Box::pin(self.generic("eLowerBound", lower)).await?;
                        result.children.push(child);
                        traversal.tree().await;
                    };
                }
            }
        }
        Ok(result)
    }
    async fn signature(&self, result: &mut Node, slot: &Feature) -> Result<(), ImportError> {
        let traversal = self.traversal;
        match &slot.ty.kind {
            TypeSyntaxKind::Named(name, args, quoted)
                if args.is_empty() && !self.scope.contains_key(name) =>
            {
                attribute(
                    result,
                    "eType",
                    self.reference(name, &slot.ty.span, *quoted)?,
                    traversal,
                )
                .await
            }
            TypeSyntaxKind::External(href, args) if args.is_empty() => {
                attribute(result, "eType", href, traversal).await
            }
            _ => {
                let child = Box::pin(self.generic("eGenericType", &slot.ty)).await?;
                result.children.push(child);
                traversal.tree().await;
            }
        }
        attribute(result, "lowerBound", slot.lower.to_string(), traversal).await;
        attribute(
            result,
            "upperBound",
            slot.upper.map_or("-1".into(), |n| n.to_string()),
            traversal,
        )
        .await;
        for (flag, key, value) in [
            ("unordered", "ordered", "false"),
            ("nonunique", "unique", "false"),
            ("containment", "containment", "true"),
            ("derived", "derived", "true"),
            ("readonly", "changeable", "false"),
            ("transient", "transient", "true"),
            ("volatile", "volatile", "true"),
            ("unsettable", "unsettable", "true"),
            ("id", "iD", "true"),
            ("noresolve", "resolveProxies", "false"),
        ] {
            if slot.flags.contains(flag) {
                attribute(result, key, value, traversal).await;
            }
        }
        Box::pin(annotations(result, &slot.annotations, traversal)).await;
        Ok(())
    }
    async fn parameters(
        &self,
        result: &mut Node,
        parameters: &[TypeParameterSyntax],
    ) -> Result<(), ImportError> {
        let traversal = self.traversal;
        for parameter in parameters {
            let mut p = node("eTypeParameters", None, &parameter.span, traversal).await;
            attribute(&mut p, "name", &parameter.name, traversal).await;
            for bound in &parameter.bounds {
                {
                    let child = Box::pin(self.generic("eBounds", bound)).await?;
                    p.children.push(child);
                    traversal.tree().await;
                };
            }
            Box::pin(annotations(&mut p, &parameter.annotations, traversal)).await;
            {
                result.children.push(p);
                traversal.tree().await;
            };
        }
        Ok(())
    }
}
async fn annotations(
    parent: &mut Node,
    source: &[AnnotationSyntax],
    traversal: &preparation::Traversal,
) {
    for a in source {
        let mut n = node("eAnnotations", None, &a.span, traversal).await;
        attribute(&mut n, "source", &a.source, traversal).await;
        if !a.references.is_empty() {
            attribute(&mut n, "references", a.references.join(" "), traversal).await;
        }
        for (key, value, span) in &a.details {
            let mut detail = node("details", None, span, traversal).await;
            attribute(&mut detail, "key", key, traversal).await;
            attribute(&mut detail, "value", value, traversal).await;
            {
                n.children.push(detail);
                traversal.tree().await;
            };
        }
        Box::pin(annotations(&mut n, &a.annotations, traversal)).await;
        {
            parent.children.push(n);
            traversal.tree().await;
        };
    }
}
async fn metadata(n: &mut Node, metadata: &ClassifierMetadata, traversal: &preparation::Traversal) {
    if let Some(identity) = &metadata.identity {
        attribute(n, "xmi:id", identity, traversal).await;
    }
    if let Some(value) = &metadata.instance_class {
        attribute(n, "instanceClassName", value, traversal).await;
    }
    if let Some(value) = &metadata.instance_type {
        attribute(n, "instanceTypeName", value, traversal).await;
    }
}
async fn scope(
    parent: &Scope,
    parameters: &[TypeParameterSyntax],
    owner: &str,
    traversal: &preparation::Traversal,
) -> Scope {
    let mut scope = Scope::new();
    for (name, value) in parent {
        scope.insert(name.clone(), value.clone());
        traversal.tree().await;
    }
    for (index, parameter) in parameters.iter().enumerate() {
        scope.insert(
            parameter.name.clone(),
            format!("{owner}/@eTypeParameters.{index}"),
        );
        traversal.tree().await;
    }
    scope
}
pub(super) async fn build(
    parsed: &Structure,
    traversal: &preparation::Traversal,
) -> Result<Node, ImportError> {
    async fn names(
        p: &Structure,
        prefix: &str,
        all: &mut BTreeSet<String>,
        traversal: &preparation::Traversal,
    ) {
        for name in p
            .classes
            .iter()
            .map(|c| &c.name)
            .chain(p.enums.iter().map(|e| &e.name))
            .chain(p.datatypes.iter().map(|d| &d.name))
        {
            all.insert(format!("{prefix}{name}"));
            traversal.tree().await;
        }
        for child in &p.packages {
            traversal.tree().await;
            Box::pin(names(
                child,
                &format!("{prefix}{}/", child.package),
                all,
                traversal,
            ))
            .await;
        }
    }
    if parsed.resource {
        let mut result = node(
            "XMI",
            Some(crate::definitions::ecore::XMI),
            &parsed.span,
            traversal,
        )
        .await;
        for (index, child) in parsed.packages.iter().enumerate() {
            let mut all = BTreeSet::new();
            Box::pin(names(child, "", &mut all, traversal)).await;
            {
                let child = Box::pin(package(
                    child,
                    &all,
                    "",
                    &format!("#/{index}"),
                    true,
                    traversal,
                ))
                .await?;
                result.children.push(child);
                traversal.tree().await;
            };
        }
        Ok(result)
    } else {
        let mut all = BTreeSet::new();
        Box::pin(names(parsed, "", &mut all, traversal)).await;
        Box::pin(package(parsed, &all, "", "#/", true, traversal)).await
    }
}
async fn package(
    parsed: &Structure,
    names: &BTreeSet<String>,
    prefix: &str,
    path: &str,
    root: bool,
    traversal: &preparation::Traversal,
) -> Result<Node, ImportError> {
    let mut result = node(
        if root { "EPackage" } else { "eSubpackages" },
        if root { Some(ECORE) } else { None },
        &parsed.span,
        traversal,
    )
    .await;
    attribute(&mut result, "name", &parsed.package, traversal).await;
    attribute(&mut result, "nsURI", &parsed.namespace, traversal).await;
    attribute(&mut result, "nsPrefix", &parsed.prefix, traversal).await;
    if let Some(identity) = &parsed.identity {
        attribute(&mut result, "xmi:id", identity, traversal).await;
    }
    for (index, class) in parsed.classes.iter().enumerate() {
        let owner = format!("{path}/@eClassifiers.{index}");
        let context = Context {
            names,
            traversal,
            package: prefix.into(),
            scope: scope(&Scope::new(), &class.type_parameters, &owner, traversal).await,
        };
        let mut c = node("eClassifiers", None, &class.span, traversal).await;
        attribute(&mut c, "xsi:type", "ecore:EClass", traversal).await;
        attribute(&mut c, "name", &class.name, traversal).await;
        if class.abstract_type {
            attribute(&mut c, "abstract", "true", traversal).await;
        }
        if class.interface_type {
            attribute(&mut c, "interface", "true", traversal).await;
        }
        metadata(&mut c, &class.metadata, traversal).await;
        for feature in &class.features {
            let mut f = node("eStructuralFeatures", None, &feature.span, traversal).await;
            attribute(
                &mut f,
                "xsi:type",
                if feature.reference {
                    "ecore:EReference"
                } else {
                    "ecore:EAttribute"
                },
                traversal,
            )
            .await;
            attribute(&mut f, "name", &feature.name, traversal).await;
            context.signature(&mut f, feature).await?;
            if let Some(default) = &feature.default {
                let value = match default {
                    DefaultSyntax::Literal(v) => v,
                    DefaultSyntax::Enum { owner, literal } => {
                        let mut enumeration = None;
                        for candidate in &parsed.enums {
                            traversal.tree().await;
                            if candidate.name == *owner {
                                enumeration = Some(candidate);
                                break;
                            }
                        }
                        let enumeration = enumeration
                            .ok_or_else(|| failure(&feature.span, "Unknown enum default type"))?;
                        if !matches!(&feature.ty.kind, TypeSyntaxKind::Named(name, args, _) if name == owner && args.is_empty())
                        {
                            return Err(failure(
                                &feature.span,
                                "Enum default does not match attribute type",
                            ));
                        }
                        let mut declared = None;
                        for candidate in &enumeration.literals {
                            traversal.tree().await;
                            if candidate.name == *literal {
                                declared = Some(candidate);
                                break;
                            }
                        }
                        let declared = declared.ok_or_else(|| {
                            failure(&feature.span, "Unknown enum default literal")
                        })?;
                        declared.literal.as_ref().unwrap_or(literal)
                    }
                };
                attribute(&mut f, "defaultValueLiteral", value, traversal).await;
            }
            if let Some(opposite) = &feature.opposite {
                attribute(&mut f, "eOpposite", opposite, traversal).await;
            }
            {
                c.children.push(f);
                traversal.tree().await;
            };
        }
        for (oi, op) in class.operations.iter().enumerate() {
            let context = Context {
                names,
                traversal,
                package: prefix.into(),
                scope: scope(
                    &context.scope,
                    &op.type_parameters,
                    &format!("{owner}/@eOperations.{oi}"),
                    traversal,
                )
                .await,
            };
            let mut o = node("eOperations", None, &op.span, traversal).await;
            attribute(&mut o, "name", &op.name, traversal).await;
            if let Some(result) = &op.result {
                context.signature(&mut o, result).await?;
            }
            for parameter in &op.parameters {
                let mut p = node("eParameters", None, &parameter.span, traversal).await;
                attribute(&mut p, "name", &parameter.name, traversal).await;
                context.signature(&mut p, parameter).await?;
                {
                    o.children.push(p);
                    traversal.tree().await;
                };
            }
            context.parameters(&mut o, &op.type_parameters).await?;
            for exception in &op.exceptions {
                {
                    let child = Box::pin(context.generic("eGenericExceptions", exception)).await?;
                    o.children.push(child);
                    traversal.tree().await;
                };
            }
            Box::pin(annotations(&mut o, &op.annotations, traversal)).await;
            {
                c.children.push(o);
                traversal.tree().await;
            };
        }
        context.parameters(&mut c, &class.type_parameters).await?;
        let mut plain_parents = Vec::new();
        for parent in &class.parents {
            traversal.tree().await;
            match &parent.kind {
                TypeSyntaxKind::Named(name, arguments, quoted)
                    if arguments.is_empty() && !context.scope.contains_key(name) =>
                {
                    plain_parents.push(context.reference(name, &parent.span, *quoted)?);
                }
                TypeSyntaxKind::External(href, arguments) if arguments.is_empty() => {
                    plain_parents.push(href.clone())
                }
                _ => {
                    let child = Box::pin(context.generic("eGenericSuperTypes", parent)).await?;
                    c.children.push(child);
                    traversal.tree().await;
                }
            }
        }
        if !plain_parents.is_empty() {
            attribute(&mut c, "eSuperTypes", plain_parents.join(" "), traversal).await;
        }
        Box::pin(annotations(&mut c, &class.annotations, traversal)).await;
        {
            result.children.push(c);
            traversal.tree().await;
        };
    }
    for en in &parsed.enums {
        let mut e = node("eClassifiers", None, &en.span, traversal).await;
        attribute(&mut e, "xsi:type", "ecore:EEnum", traversal).await;
        attribute(&mut e, "name", &en.name, traversal).await;
        metadata(&mut e, &en.metadata, traversal).await;
        if !en.serializable {
            attribute(&mut e, "serializable", "false", traversal).await;
        }
        for literal in &en.literals {
            let mut l = node("eLiterals", None, &literal.span, traversal).await;
            attribute(&mut l, "name", &literal.name, traversal).await;
            attribute(&mut l, "value", literal.value.to_string(), traversal).await;
            if let Some(value) = &literal.literal {
                attribute(&mut l, "literal", value, traversal).await;
            }
            Box::pin(annotations(&mut l, &literal.annotations, traversal)).await;
            {
                e.children.push(l);
                traversal.tree().await;
            };
        }
        Box::pin(annotations(&mut e, &en.annotations, traversal)).await;
        {
            result.children.push(e);
            traversal.tree().await;
        };
    }
    for (index, datatype) in parsed.datatypes.iter().enumerate() {
        let owner = format!(
            "{path}/@eClassifiers.{}",
            parsed.classes.len() + parsed.enums.len() + index
        );
        let context = Context {
            names,
            traversal,
            package: prefix.into(),
            scope: scope(&Scope::new(), &datatype.type_parameters, &owner, traversal).await,
        };
        let mut d = node("eClassifiers", None, &datatype.span, traversal).await;
        attribute(&mut d, "xsi:type", "ecore:EDataType", traversal).await;
        attribute(&mut d, "name", &datatype.name, traversal).await;
        metadata(&mut d, &datatype.metadata, traversal).await;
        if !datatype.serializable {
            attribute(&mut d, "serializable", "false", traversal).await;
        }
        context
            .parameters(&mut d, &datatype.type_parameters)
            .await?;
        Box::pin(annotations(&mut d, &datatype.annotations, traversal)).await;
        {
            result.children.push(d);
            traversal.tree().await;
        };
    }
    for (index, child) in parsed.packages.iter().enumerate() {
        {
            let child = Box::pin(package(
                child,
                names,
                &format!("{prefix}{}/", child.package),
                &format!("{path}/@eSubpackages.{index}"),
                false,
                traversal,
            ))
            .await?;
            result.children.push(child);
            traversal.tree().await;
        };
    }
    Box::pin(annotations(&mut result, &parsed.annotations, traversal)).await;
    Ok(result)
}
