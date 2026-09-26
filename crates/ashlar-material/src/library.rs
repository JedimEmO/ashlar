//! The graph library: a key to a graph, and the subgraph resolution that needs it.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    GraphError, GraphParam, GraphWarning, Material, MaterialGraph, Period, ValueType, graph,
    nodes::{Resolved, Subgraph, SurfaceOutput},
    require,
};

/// Independently authored material graphs, keyed by content key.
///
/// The interchange form is RON, the way a material palette's is, and a
/// [`crate::nodes::Subgraph`] node names a key in here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialGraphLibrary {
    /// Content key to graph. A graph's own `id` need not be its key, but
    /// [`Self::insert`] uses it as one.
    pub graphs: BTreeMap<String, MaterialGraph>,
}

impl MaterialGraphLibrary {
    /// Add a graph under its own id, returning whatever that key held.
    pub fn insert(&mut self, graph: MaterialGraph) -> Option<MaterialGraph> {
        self.graphs.insert(graph.id.clone(), graph)
    }

    /// Look up a graph by key.
    pub fn get(&self, key: &str) -> Option<&MaterialGraph> {
        self.graphs.get(key)
    }

    /// Merge another library into this one, refusing a key both hold.
    ///
    /// This is how a game takes [`stdlib`](crate::stdlib::graphs): the shipped
    /// compounds and the game's own graphs are two libraries authored by two
    /// people who never met, and the only thing they share is a namespace. A
    /// collision is refused rather than resolved because either resolution is
    /// wrong — replacing means a game silently loses the graph it wrote, and
    /// keeping means it silently gets a compound it did not write — and the
    /// author who hit one only has to rename something.
    ///
    /// Nothing is inserted until every key is known to be new, so a refused
    /// merge leaves this library exactly as it was.
    ///
    /// ```
    /// use ashlar_material::{MaterialGraph, MaterialGraphLibrary, stdlib};
    ///
    /// let mut library = MaterialGraphLibrary::default();
    /// library.insert(MaterialGraph::builder("game:wall").into_graph());
    /// library.extend(&stdlib::graphs())?;
    /// assert!(library.get("weathering:dirt_dust").is_some());
    ///
    /// // A second merge is the same keys again, and says so by key: the first
    /// // one the two libraries have in common, which is where the check stops.
    /// let error = library.extend(&stdlib::graphs()).unwrap_err();
    /// let first = stdlib::graphs().graphs.keys().next().expect("the library ships graphs").clone();
    /// assert_eq!(error.path, format!("graphs[{first}]"));
    /// # Ok::<(), ashlar_material::GraphError>(())
    /// ```
    pub fn extend(&mut self, other: &Self) -> Result<(), GraphError> {
        for key in other.graphs.keys() {
            require(
                !self.graphs.contains_key(key),
                &format!("graphs[{key}]"),
                "this library already holds a graph under that key, and merging one library into \
                 another may not decide which of two graphs an author meant; rename one of them",
            )?;
        }
        self.graphs.extend(
            other
                .graphs
                .iter()
                .map(|(key, graph)| (key.clone(), graph.clone())),
        );
        Ok(())
    }

    /// Validate one graph, resolving any subgraph it instances.
    ///
    /// Errors carry the path within the library, so a failure inside an
    /// instanced graph reads `graphs[key].nodes[id].inputs[name]`.
    pub fn build(&self, key: &str) -> Result<Material, GraphError> {
        let graph = self
            .graphs
            .get(key)
            .ok_or_else(|| GraphError::new("graphs", format!("unknown graph {key:?}")))?;
        let mut resolver = Resolver::new(self);
        resolver.stack.push(key.to_owned());
        resolver
            .build(graph.clone())
            .map_err(|error| error.under(&format!("graphs[{key}]")))
    }

    /// Validate every graph in the library.
    pub fn build_all(&self) -> Result<BTreeMap<String, Material>, GraphError> {
        self.graphs
            .keys()
            .map(|key| Ok((key.clone(), self.build(key)?)))
            .collect()
    }

    /// Whether every graph in the library validates, discarding the result.
    pub fn check(&self) -> Result<(), GraphError> {
        self.build_all().map(|_| ())
    }
}

/// What a build needs to answer questions about other graphs: the library, the
/// stack of graphs currently being built, and what has already been built.
pub(crate) struct Resolver<'a> {
    library: &'a MaterialGraphLibrary,
    /// Graph keys being built, outermost first. A key that is already here is
    /// a graph that includes itself.
    stack: Vec<String>,
    /// Every instance built on the way, so a graph instanced twice under the
    /// same bindings is built once and the material that instanced it can carry
    /// it away for lowering.
    built: BTreeMap<InstanceKey, Arc<Material>>,
    /// Warnings raised inside instanced graphs, rooted at the key they came
    /// from, waiting for the graph that instanced them to collect them.
    warnings: Vec<GraphWarning>,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(library: &'a MaterialGraphLibrary) -> Self {
        Self {
            library,
            stack: Vec::new(),
            built: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    pub(crate) fn build(&mut self, graph: MaterialGraph) -> Result<Material, GraphError> {
        graph::build(graph, self, None)
    }

    /// Everything instanced graphs have warned about since the last call.
    ///
    /// A graph is built once however often it is instanced, so each warning is
    /// taken exactly once, by the graph being built when it was raised.
    pub(crate) fn take_warnings(&mut self) -> Vec<GraphWarning> {
        std::mem::take(&mut self.warnings)
    }

    /// One instance this build has already validated.
    ///
    /// A [`Material`](crate::Material) keeps the instances it holds, because
    /// lowering inlines them and a node holds its graph only by key. They are
    /// built here once however often they are instanced under the same
    /// bindings.
    pub(crate) fn built(&self, key: &InstanceKey) -> Option<&Arc<Material>> {
        self.built.get(key)
    }

    /// A key the library does not hold, said in a way that distinguishes a
    /// missing graph from a missing library.
    ///
    /// [`MaterialGraph::build`](crate::MaterialGraph::build) resolves against an
    /// empty library, so a subgraph fails there however right its key is; that
    /// author is looking for the wrong thing if the message only names the key.
    fn unknown(&self, key: &str, path: &str) -> GraphError {
        let reason = if self.library.graphs.is_empty() {
            format!(
                "unknown graph {key:?}: this graph was built against no library at all, and a \
                 subgraph needs the library that holds what it instances; build it with \
                 `MaterialGraph::build_in` or `MaterialGraphLibrary::build`"
            )
        } else {
            format!("unknown graph {key:?}")
        };
        GraphError::new(format!("{path}.graph"), reason)
    }

    /// Resolve one subgraph node: check what it binds, build the instance it
    /// asks for, and answer with that instance and the period of the output it
    /// reads.
    ///
    /// Building the instanced graph is what detects recursion: a key that is
    /// already on the stack is a graph that includes itself. Recursion is by
    /// key alone and not by instance, because a graph that reaches itself under
    /// one binding reaches itself under every other one too, and the cycle to
    /// report is the one the author wrote.
    ///
    /// `inputs` is what reached this node's own ports, already resolved by the
    /// graph being built, which is where the type, period and lattice crossing
    /// the boundary come from.
    pub(crate) fn subgraph_instance(
        &mut self,
        node: &Subgraph,
        inputs: &[Resolved],
        path: &str,
    ) -> Result<(InstanceKey, Period), GraphError> {
        let key = node.graph.clone();
        if let Some(position) = self.stack.iter().position(|open| *open == key) {
            let mut cycle: Vec<&str> = self.stack[position..].iter().map(String::as_str).collect();
            cycle.push(&key);
            return Err(GraphError::new(
                format!("{path}.graph"),
                format!("recursive subgraph: {}", cycle.join(" -> ")),
            ));
        }
        // Copied out of `self` rather than borrowed from it, so that building
        // the instance below still has the resolver to itself.
        let library = self.library;
        let source = library.get(&key).ok_or_else(|| self.unknown(&key, path))?;
        let bindings = checked_bindings(&key, source, node, inputs, path)?;
        let instance = InstanceKey::new(&key, &bindings);
        if !self.built.contains_key(&instance) {
            self.stack.push(key.clone());
            let material = graph::build(source.clone(), self, Some(&bindings));
            self.stack.pop();
            let material = material.map_err(|error| inner_error(error, &instance, path))?;
            // What the instanced graph found suspicious is the outer author's
            // to read too: they are the one who chose to instance it.
            self.warnings
                .extend(material.warnings().iter().map(|warning| {
                    GraphWarning::new(
                        format!("graphs[{instance}].{}", warning.path),
                        warning.message.clone(),
                    )
                }));
            self.built.insert(instance.clone(), Arc::new(material));
        }
        let Some(instanced) = self.built.get(&instance) else {
            return Err(self.unknown(&key, path));
        };
        // A name the instanced graph does not have would otherwise bind
        // nothing and leave the default in place, which reads as the node
        // having no effect at all.
        for (name, value) in &node.params {
            let path = format!("{path}.params[{name}]");
            let Some(expected) = instanced.param(name).map(|param| param.value.value_type()) else {
                return Err(GraphError::new(
                    path,
                    format!("graph {key:?} has no parameter {name:?}"),
                ));
            };
            let found = value.value_type();
            require(
                found == expected,
                &path,
                &format!("graph {key:?} takes a {expected} for {name:?}, not a {found}"),
            )?;
        }
        let port = instanced.output_port(node.output.port()).ok_or_else(|| {
            GraphError::new(
                format!("{path}.output"),
                format!(
                    "graph {key:?} binds no {}",
                    missing(&node.output, instanced)
                ),
            )
        })?;
        Ok((instance, port.period))
    }
}

/// One instance of a library graph: the key it was authored under, and what an
/// instance wired into its inputs.
///
/// An instance is specialised per binding signature rather than per key,
/// because the inner graph is inferred once and a caller's field changes what
/// that inference answers: a field of period eight read through an inner
/// transform of scale two lays sixteen cells, and only inferring the graph
/// again under the binding says so. So this is what the build caches an
/// instance under, what a [`Material`] carries away, and what
/// [`ir::inline`](crate::ir) looks the instance back up by.
///
/// The bindings are carried whole rather than as a hash of them. What this key
/// decides is which inference a node reads, so two nodes that bound different
/// fields and compared equal here would not cost a slow lookup: the second
/// would silently take the first's instance, and its material would report a
/// period and a lattice that nothing in it has.
///
/// The [`Display`](std::fmt::Display) form is the graph key alone when nothing
/// is bound — which is every instance that existed before inputs did, so the
/// error paths inside one read as they always have — and `key@<hash>`
/// otherwise, so that a diagnostic inside an instance says which instance
/// without carrying every binding in every path it prints.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct InstanceKey {
    /// The library key of the graph instanced.
    graph: String,
    /// The bindings as [`Bindings::signature`] writes them, or `None` when
    /// there are none.
    signature: Option<String>,
}

impl InstanceKey {
    /// The instance of a graph that binds nothing, which is what every
    /// [`Subgraph`] took before a graph could declare an input.
    pub(crate) fn unbound(graph: impl Into<String>) -> Self {
        Self {
            graph: graph.into(),
            signature: None,
        }
    }

    fn new(graph: &str, bindings: &Bindings) -> Self {
        Self {
            graph: graph.to_owned(),
            signature: (!bindings.is_empty()).then(|| bindings.signature()),
        }
    }
}

impl std::fmt::Display for InstanceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.signature {
            Some(signature) => write!(f, "{}@{:016x}", self.graph, digest(signature)),
            None => f.write_str(&self.graph),
        }
    }
}

/// What one instance wires into the inputs of the graph it instances.
///
/// A field arrives as three facts rather than as a value: the type it carries,
/// how it tiles, and how fine a lattice it lays. The period and the lattice are
/// what the inner graph is inferred against. The type is not — an input
/// presents the type it declared whatever reaches it, and the conversion is
/// made at lowering, the way any port converts what reaches it — but it is part
/// of what an instance is all the same, so that one instance stands for exactly
/// one set of fields; the cost is a second instance when two nodes bind the
/// same shape at two widths, and the two inline to the same arithmetic. The
/// value itself crosses at lowering, where the inner graph is inlined into the
/// arena that instanced it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bindings {
    inputs: BTreeMap<String, Binding>,
}

impl Bindings {
    /// Whether this instance binds nothing, which is every instance of a graph
    /// that declares no input.
    pub(crate) fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// What reached one input, or `None` for one the instance left alone.
    pub(crate) fn get(&self, name: &str) -> Option<Binding> {
        self.inputs.get(name).copied()
    }

    /// The bindings written out as one text, which is what an instance is
    /// identified by.
    ///
    /// Every name is prefixed with its length, because an input name is a
    /// string its author chose and may hold the very punctuation this text
    /// separates entries with: a name spelled `x=Float 8x8 8x8;y` would
    /// otherwise read as the two entries it looks like, and two different sets
    /// of bindings would write one text. With the length in front, the text
    /// can be read back apart, so bindings that differ anywhere differ here.
    /// The order is the map's, which is by name, so bindings that agree always
    /// write the same text.
    fn signature(&self) -> String {
        use std::fmt::Write as _;
        let mut text = String::new();
        for (name, binding) in &self.inputs {
            let [u, v] = binding.lattice;
            let _ = write!(
                text,
                "{}:{name}={} {} {u}x{v};",
                name.len(),
                binding.value_type,
                binding.period
            );
        }
        text
    }
}

/// A short stable name for a binding signature, for the paths a diagnostic
/// prints.
///
/// The same in every run and every build of the crate, because it reaches an
/// author: it names the instance in every error path inside one, and a key that
/// moved between two runs of the same bake would make two diagnostics of one
/// fault. So it is FNV-1a spelled out, a multiply and an xor, rather than a
/// dependency or a hasher whose bytes are a Rust version's to change. Nothing
/// is decided by it; what an instance *is* is the signature itself.
fn digest(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One field as it arrives at an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    /// What the field carries, before the input converts it to its declared
    /// type the way any port converts what reaches it.
    pub(crate) value_type: ValueType,
    /// How the field tiles, which the input carries into the inner graph.
    pub(crate) period: Period,
    /// How fine a lattice the field lays, which the inner graph's resamplers
    /// multiply and its own [`Material::finest_lattice`] then reports.
    pub(crate) lattice: [u32; 2],
}

/// The instance one [`Subgraph`] node asks for, from the node and the fields
/// that reached its ports.
///
/// Both halves of the pipeline need this answer and must agree on it: the build
/// caches an instance under it and hands the material the same key, and the
/// lowering looks the instance back up by it when it inlines the node. So it is
/// derived here, once, and neither side derives it again its own way.
pub(crate) fn instance_key(node: &Subgraph, inputs: &[Resolved]) -> InstanceKey {
    InstanceKey::new(&node.graph, &bindings(node, inputs))
}

/// The fields a node wired into the inputs it names, by name.
///
/// The one place a [`Bindings`] is made, so that the key the build caches an
/// instance under and the key the lowering looks it up by cannot differ. A name
/// whose port did not resolve is left out rather than guessed at; the build
/// refuses that node before it ever reaches a key.
fn bindings(node: &Subgraph, inputs: &[Resolved]) -> Bindings {
    let inputs = node
        .inputs
        .keys()
        .filter_map(|name| Some((name.clone(), binding(name, inputs)?)))
        .collect();
    Bindings { inputs }
}

/// What reached the port of one bound name, out of a node's resolved inputs.
fn binding(name: &str, inputs: &[Resolved]) -> Option<Binding> {
    inputs
        .iter()
        .find(|input| input.name.as_ref() == name)
        .map(|input| Binding {
            value_type: input.value_type,
            period: input.period,
            lattice: input.lattice,
        })
}

/// What a node binds to the inputs of the graph it instances, checked against
/// what that graph declares.
///
/// The graph is read before it is built, because the bindings are what it is
/// built under. Its inline nodes are hoisted first: an input written inside
/// another node is still one the graph takes, and it would not be in the map
/// until a build put it there.
fn checked_bindings(
    key: &str,
    source: &MaterialGraph,
    node: &Subgraph,
    inputs: &[Resolved],
    path: &str,
) -> Result<Bindings, GraphError> {
    if node.inputs.is_empty() {
        return Ok(Bindings::default());
    }
    let mut flattened = source.clone();
    // A graph whose hoisted ids collide is refused by its own build, which is
    // where that belongs; here it only means the declarations read are the ones
    // that survived, and the build below says the rest.
    let _ = graph::flatten(&mut flattened);
    let declared = flattened.inputs();
    for name in node.inputs.keys() {
        let path = format!("{path}.inputs[{name}]");
        // A name the instanced graph does not declare would otherwise wire a
        // field into nothing at all, leaving the input on its default: the node
        // would look wired and the picture would be the one it had before.
        let Some(input) = declared.get(name.as_str()) else {
            return Err(GraphError::new(
                path,
                format!("graph {key:?} has no input {name:?}; {}", takes(&declared)),
            ));
        };
        let expected = input.value_type;
        let Some(found) = binding(name, inputs) else {
            return Err(GraphError::new(path, "input does not resolve to a value"));
        };
        require(
            found.value_type.converts_to(expected),
            &path,
            &format!(
                "graph {key:?} takes a {expected} for input {name:?}, not a {}",
                found.value_type
            ),
        )?;
    }
    Ok(bindings(node, inputs))
}

/// The inputs a graph declares, for the message that says a name is not one.
fn takes(declared: &BTreeMap<&str, &crate::nodes::GraphInput>) -> String {
    if declared.is_empty() {
        return "it declares none".to_owned();
    }
    let names: Vec<String> = declared
        .iter()
        .map(|(name, input)| format!("{name:?} ({})", input.value_type))
        .collect();
    format!("it declares {}", names.join(", "))
}

/// A failure inside an instance, rooted where the author can do something
/// about it.
///
/// An instance that binds nothing is the graph itself, so the error stays
/// inside it, under the key it came from, exactly as it always has. An instance
/// that binds something may fail *because* of what was bound — a field that
/// does not tile is the plain case — and the node that bound it is somewhere
/// else entirely, in a graph whose author may never have opened the one that
/// broke. So that error is reported at the node, and carries the inner path in
/// its message rather than in its own.
fn inner_error(error: GraphError, instance: &InstanceKey, path: &str) -> GraphError {
    let inner = error.under(&format!("graphs[{instance}]"));
    if instance.signature.is_none() {
        return inner;
    }
    GraphError::new(
        path,
        format!(
            "graph {:?} does not build with the fields this node binds: {inner}",
            instance.graph
        ),
    )
}

/// What a graph does not bind, named the way the author can act on.
///
/// An extra is the case worth spending words on: the six channels are a closed
/// set an author can read off the type, and an export is a name somebody chose
/// inside a graph they may not have opened, so the message says what that graph
/// does export.
fn missing(output: &SurfaceOutput, instanced: &Material) -> String {
    match output {
        SurfaceOutput::Height => "height".to_owned(),
        SurfaceOutput::Emissive => "emissive".to_owned(),
        SurfaceOutput::Extra(name) => {
            format!("extra output {name:?}; {}", exports(instanced))
        }
        // The other four are always bound, so this only reads in a message
        // nothing can produce today.
        other => other.port().to_owned(),
    }
}

/// The auxiliary masks a graph exports, for the message that says a name is not
/// one of them.
fn exports(instanced: &Material) -> String {
    let names = &instanced.graph().output.extra;
    if names.is_empty() {
        return "it exports none".to_owned();
    }
    let names: Vec<String> = names.keys().map(|name| format!("{name:?}")).collect();
    format!("it exports {}", names.join(", "))
}
