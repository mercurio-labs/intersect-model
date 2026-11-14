//! Shared structural shape and substitution algorithms over fixed current inputs.
//! Locators and active frames own no references into movable roots/environments.
//! Individual metadata copying, comparison, allocation and cleanup remain separate.
use super::*;
const BINDING_EDGE: usize = usize::MAX;
#[derive(Clone, Copy)]
struct Route {
    parent: usize,
    edge: usize,
    depth: usize,
    // Pending-stack link before visiting; child-admission cursor afterward.
    next: usize,
}
const _: () = assert!(std::mem::size_of::<Route>() <= 32);
enum ShapePhase {
    Begin,
    Next,
    Children,
    Complete,
    Failed,
}
pub(crate) struct ShapePreparation {
    routes: Vec<Route>,
    head: usize,
    current: usize,
    count: usize,
    phase: ShapePhase,
}
fn invalid_path() -> PreparationError {
    "Invalid retained structural type path".into()
}
fn child(input: &Type, edge: usize) -> Result<Option<&Type>, PreparationError> {
    match input {
        Type::Classifier { arguments, .. } => Ok(arguments.get(edge)),
        Type::Wildcard { upper, lower } => Ok(match edge {
            0 => upper.as_deref(),
            1 => lower.as_deref(),
            _ => None,
        }),
        _ => Err(invalid_path()),
    }
}
impl ShapePreparation {
    pub(crate) fn begin() -> Self {
        Self {
            routes: vec![],
            head: BINDING_EDGE,
            current: BINDING_EDGE,
            count: 0,
            phase: ShapePhase::Begin,
        }
    }
    fn input<'a>(&self, root: &'a Type, mut route: usize) -> Result<&'a Type, PreparationError> {
        // Only already depth-checked nodes are resolved. Address lookup has the
        // original fixed depth ceiling and never rescans the complete tree.
        let mut edges = [0; MAX_DEPTH + 1];
        let mut depth = 0;
        loop {
            let current = self.routes.get(route).ok_or_else(invalid_path)?;
            if current.parent == BINDING_EDGE {
                break;
            }
            *edges.get_mut(depth).ok_or_else(invalid_path)? = current.edge;
            depth += 1;
            route = current.parent;
        }
        let mut input = root;
        for edge in edges[..depth].iter().rev() {
            input = child(input, *edge)?.ok_or_else(invalid_path)?;
        }
        Ok(input)
    }
    pub(crate) fn finish(self) -> Result<(), PreparationError> {
        if matches!(self.phase, ShapePhase::Complete) {
            Ok(())
        } else {
            Err("Structural shape preparation is incomplete or failed".into())
        }
    }
    pub(crate) fn advance(
        &mut self,
        root: &Type,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        let phase = std::mem::replace(&mut self.phase, ShapePhase::Failed);
        self.phase = match phase {
            ShapePhase::Begin => {
                reserve_preparation(observation, 32, &[])?;
                self.routes.push(Route {
                    parent: BINDING_EDGE,
                    edge: 0,
                    depth: 0,
                    next: BINDING_EDGE,
                });
                self.head = 0;
                ShapePhase::Next
            }
            ShapePhase::Next => {
                if self.head == BINDING_EDGE {
                    ShapePhase::Complete
                } else {
                    let route = self.head;
                    let current = *self.routes.get(route).ok_or_else(invalid_path)?;
                    self.head = current.next;
                    observation.work(1).map_err(PreparationError::Observation)?;
                    self.count += 1;
                    if self.count > MAX_NODES || current.depth > MAX_DEPTH {
                        return Err("Generic expression size/depth budget exceeded".into());
                    }
                    let has_children = match self.input(root, route)? {
                        Type::Classifier { arguments, raw, .. } => {
                            if *raw && !arguments.is_empty() {
                                return Err("Raw classifier cannot carry actual arguments".into());
                            }
                            true
                        }
                        Type::Wildcard { upper, lower } => {
                            if upper.is_some() && lower.is_some() {
                                return Err(
                                    "Wildcard cannot have both upper and lower bounds".into()
                                );
                            }
                            true
                        }
                        Type::Parameter { .. } | Type::RigidParameter { .. } => false,
                    };
                    if has_children {
                        self.current = route;
                        self.routes[route].next = 0;
                        ShapePhase::Children
                    } else {
                        ShapePhase::Next
                    }
                }
            }
            ShapePhase::Children => {
                let route = self.current;
                let current = *self.routes.get(route).ok_or_else(invalid_path)?;
                let input = self.input(root, route)?;
                let mut next = current.next;
                let selected = match input {
                    Type::Classifier { arguments, .. } => arguments.get(next).map(|_| next),
                    Type::Wildcard { upper, lower } => {
                        let mut selected = None;
                        while next < 2 {
                            if [upper, lower][next].is_some() {
                                selected = Some(next);
                                break;
                            }
                            next += 1;
                        }
                        selected
                    }
                    _ => return Err(invalid_path()),
                };
                if let Some(edge) = selected {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    reserve_preparation(observation, 32, &[])?;
                    self.routes[route].next = edge + 1;
                    let index = self.routes.len();
                    self.routes.push(Route {
                        parent: route,
                        edge,
                        depth: current.depth + 1,
                        next: self.head,
                    });
                    self.head = index;
                    ShapePhase::Children
                } else {
                    ShapePhase::Next
                }
            }
            ShapePhase::Complete => ShapePhase::Complete,
            ShapePhase::Failed => return Err("Structural shape preparation has failed".into()),
        };
        Ok(matches!(self.phase, ShapePhase::Complete))
    }
}
#[derive(Clone, Copy)]
enum FramePhase {
    Enter,
    Classifier,
    WildcardUpper,
    WildcardLower,
    WildcardDone,
    Return,
    ReturnActive,
    Value,
}
struct Frame {
    edge: usize,
    mode: Expansion,
    phase: FramePhase,
    value: Option<Type>,
}
const _: () = assert!(std::mem::size_of::<Frame>() <= 128);
struct ExpansionWork {
    frames: Vec<Frame>,
    active: BTreeSet<DeclarationId>,
    remaining: usize,
}
impl ExpansionWork {
    fn begin(mode: Expansion) -> Self {
        Self {
            frames: vec![Frame {
                edge: 0,
                mode,
                phase: FramePhase::Enter,
                value: None,
            }],
            active: BTreeSet::new(),
            remaining: MAX_NODES,
        }
    }
    fn input<'a>(
        &self,
        root: &'a Type,
        environment: &'a Substitution,
    ) -> Result<&'a Type, PreparationError> {
        let mut input = root;
        for frame in self.frames.iter().skip(1) {
            input = if frame.edge == BINDING_EDGE {
                let Type::Parameter { declaration } = input else {
                    return Err(invalid_path());
                };
                environment
                    .bindings
                    .get(declaration)
                    .ok_or_else(invalid_path)?
            } else {
                child(input, frame.edge)?.ok_or_else(invalid_path)?
            };
        }
        Ok(input)
    }
    fn value(&mut self, value: Type) -> Result<(), PreparationError> {
        let frame = self.frames.last_mut().ok_or_else(invalid_path)?;
        frame.value = Some(value);
        frame.phase = FramePhase::Value;
        Ok(())
    }
    fn advance(
        &mut self,
        root: &Type,
        environment: &Substitution,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        let index = self.frames.len().checked_sub(1).ok_or_else(invalid_path)?;
        let mode = self.frames[index].mode;
        match self.frames[index].phase {
            FramePhase::Enter => {
                observation.work(1).map_err(PreparationError::Observation)?;
                if index > MAX_DEPTH || self.remaining == 0 {
                    return Err("Generic substitution size/depth budget exceeded".into());
                }
                self.remaining -= 1;
                match self.input(root, environment)? {
                    input @ Type::RigidParameter { .. } => {
                        reserve_preparation_value(input, observation)?;
                        self.value(input.clone())?;
                    }
                    Type::Classifier {
                        declaration, raw, ..
                    } => {
                        reserve_preparation(observation, 128, &[])?;
                        reserve_preparation_value(declaration, observation)?;
                        self.frames[index].value = Some(Type::Classifier {
                            declaration: declaration.clone(),
                            arguments: vec![],
                            raw: *raw,
                        });
                        self.frames[index].phase = FramePhase::Classifier;
                    }
                    Type::Wildcard { .. } => {
                        reserve_preparation(observation, 128, &[])?;
                        self.frames[index].value = Some(Type::Wildcard {
                            upper: None,
                            lower: None,
                        });
                        self.frames[index].phase = FramePhase::WildcardUpper;
                    }
                    input @ Type::Parameter { declaration } => {
                        if matches!(mode, Expansion::Copy) {
                            reserve_preparation_value(input, observation)?;
                            self.value(input.clone())?;
                        } else if let Some(argument) = environment.bindings.get(declaration) {
                            if matches!(mode, Expansion::Simultaneous) {
                                self.frames[index].phase = FramePhase::Return;
                                self.frames.push(Frame {
                                    edge: BINDING_EDGE,
                                    mode: Expansion::Copy,
                                    phase: FramePhase::Enter,
                                    value: None,
                                });
                            } else if argument == input {
                                reserve_preparation_value(input, observation)?;
                                self.value(input.clone())?;
                            } else {
                                if self.active.contains(declaration) {
                                    return Err(format!(
                                        "Cyclic generic substitution at {} {:?}",
                                        declaration.source.uri, declaration.path
                                    )
                                    .into());
                                }
                                reserve_preparation(observation, 128, &[])?;
                                reserve_preparation_value(declaration, observation)?;
                                self.active.insert(declaration.clone());
                                self.frames[index].phase = FramePhase::ReturnActive;
                                self.frames.push(Frame {
                                    edge: BINDING_EDGE,
                                    mode,
                                    phase: FramePhase::Enter,
                                    value: None,
                                });
                            }
                        } else {
                            reserve_preparation_value(input, observation)?;
                            self.value(input.clone())?;
                        }
                    }
                }
            }
            FramePhase::Classifier => {
                let Some(Type::Classifier { arguments, .. }) = self.frames[index].value.as_ref()
                else {
                    return Err(invalid_path());
                };
                let next = arguments.len();
                let Type::Classifier {
                    arguments: source, ..
                } = self.input(root, environment)?
                else {
                    return Err(invalid_path());
                };
                if next < source.len() {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    reserve_preparation(observation, 32, &[])?;
                    self.frames.push(Frame {
                        edge: next,
                        mode,
                        phase: FramePhase::Enter,
                        value: None,
                    });
                } else {
                    self.frames[index].phase = FramePhase::Value;
                }
            }
            FramePhase::WildcardUpper | FramePhase::WildcardLower => {
                let edge = if matches!(self.frames[index].phase, FramePhase::WildcardUpper) {
                    0
                } else {
                    1
                };
                let Type::Wildcard { upper, lower } = self.input(root, environment)? else {
                    return Err(invalid_path());
                };
                let present = [upper, lower][edge].is_some();
                if present {
                    observation.work(1).map_err(PreparationError::Observation)?;
                    reserve_preparation(observation, 128, &[])?;
                }
                self.frames[index].phase = if edge == 0 {
                    FramePhase::WildcardLower
                } else {
                    FramePhase::WildcardDone
                };
                if present {
                    self.frames.push(Frame {
                        edge,
                        mode,
                        phase: FramePhase::Enter,
                        value: None,
                    });
                }
            }
            FramePhase::WildcardDone => self.frames[index].phase = FramePhase::Value,
            FramePhase::Value => {
                if index == 0 {
                    return Ok(true);
                }
                let value = self
                    .frames
                    .pop()
                    .and_then(|mut frame| frame.value.take())
                    .ok_or_else(invalid_path)?;
                let parent = index - 1;
                match self.frames[parent].phase {
                    FramePhase::Classifier => {
                        let Some(Type::Classifier { arguments, .. }) =
                            self.frames[parent].value.as_mut()
                        else {
                            return Err(invalid_path());
                        };
                        arguments.push(value);
                    }
                    FramePhase::WildcardLower | FramePhase::WildcardDone => {
                        let upper_selected =
                            matches!(self.frames[parent].phase, FramePhase::WildcardLower);
                        let Some(Type::Wildcard { upper, lower }) =
                            self.frames[parent].value.as_mut()
                        else {
                            return Err(invalid_path());
                        };
                        if upper_selected {
                            *upper = Some(Box::new(value));
                        } else {
                            *lower = Some(Box::new(value));
                        }
                    }
                    FramePhase::ReturnActive => {
                        let Type::Parameter { declaration } = self.input(root, environment)? else {
                            return Err(invalid_path());
                        };
                        self.active.remove(declaration);
                        self.value(value)?;
                    }
                    FramePhase::Return => self.value(value)?,
                    _ => return Err(invalid_path()),
                }
            }
            FramePhase::Return | FramePhase::ReturnActive => return Err(invalid_path()),
        }
        Ok(false)
    }
    fn finish(mut self) -> Result<Type, PreparationError> {
        self.frames
            .pop()
            .and_then(|mut frame| frame.value.take())
            .ok_or_else(invalid_path)
    }
}
enum Phase {
    Shape(ShapePreparation),
    Expand(ExpansionWork),
    Complete(ExpansionWork),
    Failed,
}
pub(crate) struct SubstitutionPreparation {
    mode: Expansion,
    phase: Phase,
}
const _: () = assert!(std::mem::size_of::<SubstitutionPreparation>() <= 80);
impl SubstitutionPreparation {
    pub(crate) fn simultaneous() -> Self {
        Self::begin(Expansion::Simultaneous)
    }
    pub(crate) fn transitive() -> Self {
        Self::begin(Expansion::Transitive)
    }
    fn begin(mode: Expansion) -> Self {
        Self {
            mode,
            phase: Phase::Shape(ShapePreparation::begin()),
        }
    }
    pub(crate) fn advance(
        &mut self,
        root: &Type,
        environment: &Substitution,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, PreparationError> {
        let phase = std::mem::replace(&mut self.phase, Phase::Failed);
        self.phase = match phase {
            Phase::Shape(mut shape) => {
                if shape.advance(root, observation)? {
                    Phase::Expand(ExpansionWork::begin(self.mode))
                } else {
                    Phase::Shape(shape)
                }
            }
            Phase::Expand(mut work) => {
                if work.advance(root, environment, observation)? {
                    Phase::Complete(work)
                } else {
                    Phase::Expand(work)
                }
            }
            Phase::Complete(work) => Phase::Complete(work),
            Phase::Failed => return Err("Structural substitution preparation has failed".into()),
        };
        Ok(matches!(self.phase, Phase::Complete(_)))
    }
    pub(crate) fn finish(self) -> Result<Type, PreparationError> {
        match self.phase {
            Phase::Complete(work) => work.finish(),
            _ => Err("Structural substitution preparation is incomplete or failed".into()),
        }
    }
}
