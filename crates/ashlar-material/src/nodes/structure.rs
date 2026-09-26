//! Structure: the nodes that are about the graph rather than about the field.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::{Lower, Lowering, NodeInputs, ValueId, ir_type};
use crate::{
    GraphError, ParamValue, Period, Value, ValueType,
    nodes::{Check, Output, ports},
    require,
};

/// A typed signal this graph takes from whatever instances it.
///
/// A graph declares its inputs as nodes rather than in a table beside them, so
/// that everything which already walks the nodes — validation, the topological
/// order, period inference, lowering — reaches an input without being taught
/// about a second kind of thing. The node is a leaf: on its own it is its
/// default literal at unit period, so a compound still builds, bakes, checks
/// and previews with nothing wired into it, and what comes out is the picture
/// its author chose the defaults for. Instanced with a field bound to this
/// name, it is that field, converted to the type it declared the way any port
/// converts what reaches it.
///
/// It is not called `Input`, which is the enum a wired port holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct GraphInput {
    /// The name a [`Subgraph`] binds a field to, unique within the graph.
    pub name: String,
    /// The type this input presents, whatever is bound to it.
    pub value_type: ValueType,
    /// What it carries unbound, which is of the declared type.
    pub default: Value,
}

impl Default for GraphInput {
    /// A nameless black float, which validation refuses: the fields that say
    /// what an input *is* have no sensible default, and serde needs one to read
    /// a partial node.
    fn default() -> Self {
        Self {
            name: String::new(),
            value_type: ValueType::Float,
            default: Value::Float(0.0),
        }
    }
}

impl GraphInput {
    /// A one-channel input, carrying `default` until an instance binds it.
    pub fn float(name: impl Into<String>, default: f32) -> Self {
        Self {
            name: name.into(),
            value_type: ValueType::Float,
            default: Value::Float(default),
        }
    }

    /// A linear-RGB input. A float bound to it arrives broadcast.
    pub fn color(name: impl Into<String>, default: [f32; 3]) -> Self {
        Self {
            name: name.into(),
            value_type: ValueType::Color,
            default: Value::Color(default),
        }
    }

    /// A coordinate or offset input, which nothing converts into.
    pub fn vec2(name: impl Into<String>, default: [f32; 2]) -> Self {
        Self {
            name: name.into(),
            value_type: ValueType::Vec2,
            default: Value::Vec2(default),
        }
    }
}

/// The period an unbound [`GraphInput`] carries, and the reason it is not free.
///
/// [`Period::UNIT`]: with nothing bound, an input is its default literal, which
/// is one value across the whole repeat and so comes back to itself once, the
/// way a parameter does. A bound input carries the period of the field bound to
/// it, which is learnt by building the instanced graph again under the binding
/// rather than by inferring this node.
pub(crate) const UNBOUND_PERIOD: Period = Period::UNIT;

ports!(GraphInput, |node| Output::Fixed(node.value_type));

impl Check for GraphInput {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        crate::name(&self.name, &format!("{path}.name"))?;
        let found = self.default.value_type();
        let declared = self.value_type;
        require(
            found == declared,
            &format!("{path}.default"),
            &format!("an input declared {declared} takes a {declared} default, not a {found}"),
        )?;
        require(
            self.default.is_finite(),
            &format!("{path}.default"),
            "a default must be finite; NaN poisons every texel it reaches",
        )
    }
}

/// Which of another graph's outputs a [`Subgraph`] reads.
///
/// A graph presents one PBR output, so instancing it means naming which of its
/// channels this node wants; the others cost nothing, because lowering keeps
/// only what the output reaches. Beside the six channels a graph may export
/// named float masks, and [`Self::Extra`] reads one of those, which is why this
/// enum owns a string and is not [`Copy`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SurfaceOutput {
    /// Linear base colour.
    #[default]
    BaseColor,
    /// Perceptual roughness.
    Roughness,
    /// Metalness.
    Metallic,
    /// Ambient occlusion.
    Occlusion,
    /// Height. Reading it from a graph that has none is an error.
    Height,
    /// Linear emission. Reading it from a graph that has none is an error.
    Emissive,
    /// One of the graph's auxiliary float masks, by the name it exported it
    /// under. Reading a name the graph does not export is an error.
    Extra(String),
}

impl SurfaceOutput {
    /// The type this output carries, which is known without the library.
    ///
    /// An extra is a mask, so it is a float whatever it was made of: the port
    /// it is bound to accepts a float, and a colour written into one arrives as
    /// its luminance there rather than here.
    pub fn value_type(&self) -> ValueType {
        match self {
            Self::BaseColor | Self::Emissive => ValueType::Color,
            Self::Roughness | Self::Metallic | Self::Occlusion | Self::Height | Self::Extra(_) => {
                ValueType::Float
            }
        }
    }

    /// The [`PbrOutput`](crate::PbrOutput) port name this output reads.
    ///
    /// An extra's name is its port name, which is why the six channels may not
    /// be exported under one of those names: the two would be one port.
    pub fn port(&self) -> &str {
        match self {
            Self::BaseColor => "base_color",
            Self::Roughness => "roughness",
            Self::Metallic => "metallic",
            Self::Occlusion => "occlusion",
            Self::Height => "height",
            Self::Emissive => "emissive",
            Self::Extra(name) => name,
        }
    }
}

/// Another graph from the library, instanced and read by output.
///
/// Inlined at lowering, so a subgraph costs nothing at runtime. A graph may not
/// include itself, directly or through another, and the library says so by path.
///
/// What it binds comes in two kinds, and the difference is when it is resolved.
/// A *parameter* is a number the instanced graph exposed, folded into constants
/// when the instance is lowered. An *input* is a whole field wired into a
/// [`GraphInput`] the instanced graph declares, so it carries a type, a period
/// and a lattice across the boundary; the instance is inferred again under what
/// was wired in, because an input of period eight read through an inner
/// transform of scale two lays sixteen cells and nothing short of inferring it
/// again says so.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Subgraph {
    /// Key of the graph in a [`MaterialGraphLibrary`](crate::MaterialGraphLibrary).
    pub graph: String,
    /// Values for that graph's parameters; absent names keep their defaults.
    pub params: BTreeMap<String, ParamValue>,
    /// Fields for that graph's inputs, by the name each [`GraphInput`]
    /// declares; absent names carry their declared default.
    pub inputs: BTreeMap<String, crate::Input>,
    /// Which of its outputs to read.
    pub output: SurfaceOutput,
}

impl Subgraph {
    /// Instance a graph and read its base colour.
    pub fn new(graph: impl Into<String>) -> Self {
        Self {
            graph: graph.into(),
            ..Self::default()
        }
    }

    /// Bind one of that graph's parameters.
    pub fn param(mut self, name: impl Into<String>, value: ParamValue) -> Self {
        self.params.insert(name.into(), value);
        self
    }

    /// Wire a field into one of that graph's inputs.
    ///
    /// The value is anything an input port takes, a node written inline
    /// included, and the name is the one a [`GraphInput`] of the instanced
    /// graph declares. A name it does not declare is refused when the graph is
    /// built, with the names it does declare, because a misspelling would
    /// otherwise leave the input on its default and read as the node having no
    /// effect.
    pub fn input(mut self, name: impl Into<String>, value: impl Into<crate::Input>) -> Self {
        self.inputs.insert(name.into(), value.into());
        self
    }

    /// Choose which output to read.
    pub fn output(mut self, output: SurfaceOutput) -> Self {
        self.output = output;
        self
    }
}

/// The one node whose ports and output type are both fields of the node rather
/// than constants, which is why it does not go through the `ports!` macro.
///
/// A port per bound input, in the order the map holds them, which is the order
/// errors report them in. Each accepts anything: what an input really takes is
/// the type the instanced graph declared for it, and that is not a question the
/// node can answer without the library, so the resolver makes the check where
/// the inner graph is known.
impl crate::nodes::Ports for Subgraph {
    fn inputs(&self) -> Vec<crate::nodes::InputPort<'_>> {
        self.inputs
            .iter()
            .map(|(name, input)| crate::nodes::InputPort {
                name: Cow::Owned(name.clone()),
                accepts: crate::nodes::Accepts::Any,
                input,
            })
            .collect()
    }

    fn inputs_mut(&mut self) -> Vec<(Cow<'static, str>, &mut crate::Input)> {
        self.inputs
            .iter_mut()
            .map(|(name, input)| (Cow::Owned(name.clone()), input))
            .collect()
    }

    fn output(&self) -> Output {
        Output::Fixed(self.output.value_type())
    }
}

impl Check for Subgraph {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        crate::name(&self.graph, &format!("{path}.graph"))?;
        for (key, value) in &self.params {
            crate::name(key, &format!("{path}.params"))?;
            require(
                value.is_finite(),
                &format!("{path}.params[{key}]"),
                "parameter value must be finite",
            )?;
        }
        // A blank input name could name no input of any graph, and the message
        // the resolver would give for it — that the graph has no input of that
        // name — would be about the graph rather than about the typo. A dot is
        // refused for a reason of the same shape: a node written inline in an
        // input is hoisted under `<id>.<name>`, so a name holding one generates
        // an id that reads as another node's port, and the collision it can
        // cause would be reported as a duplicate id rather than as the name
        // that made it.
        for key in self.inputs.keys() {
            crate::name(key, &format!("{path}.inputs"))?;
            require(
                !key.contains('.'),
                &format!("{path}.inputs[{key}]"),
                "an input name may not hold a '.': a node written inline in an input is hoisted \
                 under `<id>.<name>`, and a dot there generates an id another node could already \
                 have",
            )?;
        }
        Ok(())
    }
}

/// A note an author leaves in the graph. Ignored by every backend.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Comment {
    /// The note.
    pub text: String,
}

impl Comment {
    /// Leave a note.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

ports!(Comment, Output::None);
impl Check for Comment {}

/// Inlined by the driver rather than here.
///
/// A subgraph is the one node whose lowering needs more than the node: it holds
/// its graph by key, and what is inlined is the validated
/// [`Material`](crate::Material) the key resolved to when this graph was built.
/// [`ir::lower`](crate::ir::lower) has both and does it there, so this body is
/// only reached by a backend that walked the nodes itself.
impl Lower for Subgraph {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        cx.reject(
            "a subgraph is inlined by the lowering driver, which is where the graph it names is \
             resolved; this backend lowered the node on its own instead",
        )
    }
}

/// Whatever an instance bound to this name, or the default.
///
/// The conversion is the one a port makes of what reaches it, so a colour bound
/// to a float input arrives as its luminance; the declared type is what the
/// graph inside was inferred against, and lowering to anything else would make
/// the instruction widths disagree with it. Nothing bound is the default
/// literal, which is a graph lowered on its own and an input an instance left
/// alone.
impl Lower for GraphInput {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        match cx.bound(&self.name) {
            Some(value) => cx.convert(value, ir_type(self.value_type)),
            None => cx.value(self.default),
        }
    }
}

/// Nothing at all.
///
/// A comment has no output, so the driver never reaches it: a node with
/// [`Output::None`] is skipped before its lowering is asked for. The body is
/// here so that the vocabulary has no node without one.
impl Lower for Comment {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        cx.constant(0.0)
    }
}
