use std::{borrow::Cow, collections::BTreeMap};

use serde::{Deserialize, Serialize};

use crate::{ValidationError, name, require};

/// A value bound to a material graph parameter.
///
/// One type on both sides of the join: a definition and a binding write it,
/// and a graph's exposed parameter holds it as its default. `Int` and `Bool`
/// are authoring conveniences — a count, a seed or a switch — and what a graph
/// makes of them is the graph engine's business.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    /// One unitless channel.
    Float(f32),
    /// Linear RGB, which inside a graph is never sRGB.
    Color([f32; 3]),
    /// A count, a seed or an integer period.
    Int(i32),
    /// A switch between two branches of a graph.
    Bool(bool),
}

impl ParamValue {
    /// Whether every floating-point component is finite.
    ///
    /// A non-finite parameter poisons every texel it reaches, so validation
    /// rejects it at the definition rather than at the bake.
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Float(value) => value.is_finite(),
            Self::Color(value) => value.iter().all(|c| c.is_finite()),
            Self::Int(_) | Self::Bool(_) => true,
        }
    }

    /// The variant tag and the bits behind it, so two values can be compared
    /// and hashed where `f32` cannot.
    ///
    /// Tagged, so that a `Float` and an `Int` holding the same bits are two
    /// values, and by bits rather than by value, so that the answer is a total
    /// order. [`Binding`] is what wants it.
    fn bits(self) -> (u8, [u32; 3]) {
        match self {
            Self::Float(value) => (0, [value.to_bits(), 0, 0]),
            Self::Color(value) => (1, value.map(f32::to_bits)),
            Self::Int(value) => (2, [value.cast_unsigned(), 0, 0]),
            Self::Bool(value) => (3, [u32::from(value), 0, 0]),
        }
    }
}

/// One material slot's binding: the material key, and the graph parameters this
/// binding overrides on it.
///
/// A slot named a key and nothing else until per-instance overrides existed,
/// and most slots still do, so both forms are one form here: a bare string
/// where nothing is overridden, and `(material: "...", params: {...})` where
/// something is. A library written before this type existed reads unchanged,
/// and a binding that overrides nothing writes itself back as the string it
/// was.
///
/// What may be overridden is the parameters of the *graph* a
/// [`Surface::Graph`] or [`Surface::Shader`] names, which is what makes a
/// per-building seed or tint one material key instead of one key per building.
/// [`MaterialLibrary::check_for`] refuses an override on a surface that names
/// no graph; whether the names it sets are parameters that graph actually
/// declares is a question this crate cannot answer, because a graph lives in a
/// crate beside this one — the adapter that holds both checks it, by the same
/// path.
#[derive(Clone, Debug)]
pub struct Binding {
    /// Content key of a definition in a [`MaterialLibrary`].
    pub material: String,
    /// Values written over the graph parameters the definition's surface binds.
    /// Empty for the ordinary slot that names a material and nothing else.
    pub params: BTreeMap<String, ParamValue>,
}

impl Binding {
    /// Bind a material, overriding nothing.
    pub fn new(material: impl Into<String>) -> Self {
        Self {
            material: material.into(),
            params: BTreeMap::new(),
        }
    }

    /// Override one graph parameter on this binding.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: ParamValue) -> Self {
        self.params.insert(name.into(), value);
        self
    }

    /// Whether this binding overrides nothing, and so is the material key alone.
    pub fn is_plain(&self) -> bool {
        self.params.is_empty()
    }

    /// The material key and every override as one comparable, hashable value.
    ///
    /// The *bits* of each parameter rather than the value, because a binding
    /// is what a renderer keys its materials by and `f32` is not [`Eq`]. That
    /// is conservative in exactly one direction: `0.0` and `-0.0` are one
    /// picture and two bindings, which costs a duplicate texture set and never
    /// hands a wall somebody else's texels. A non-finite value never arrives,
    /// because validation refuses one.
    fn order(&self) -> impl Iterator<Item = (&str, (u8, [u32; 3]))> {
        self.params
            .iter()
            .map(|(name, value)| (name.as_str(), value.bits()))
    }
}

impl PartialEq for Binding {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Binding {}

impl PartialOrd for Binding {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Binding {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.material
            .cmp(&other.material)
            .then_with(|| self.order().cmp(other.order()))
    }
}

impl std::hash::Hash for Binding {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.material.hash(state);
        self.params.len().hash(state);
        for entry in self.order() {
            entry.hash(state);
        }
    }
}

impl std::fmt::Display for Binding {
    /// The material key alone where nothing is overridden, and the key with its
    /// overrides after it otherwise: `metro:stone {variation: 0.37}`.
    ///
    /// For a log line and the context of an error — a message about "material
    /// metro:stone" is ambiguous in a block of three buildings wearing one key
    /// — rather than for a document. The interchange form is
    /// [`Serialize`](serde::Serialize)'s.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.material)?;
        if self.params.is_empty() {
            return Ok(());
        }
        formatter.write_str(" {")?;
        for (index, (name, value)) in self.params.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{name}: ")?;
            match value {
                ParamValue::Float(value) => write!(formatter, "{value}")?,
                ParamValue::Color([r, g, b]) => write!(formatter, "({r}, {g}, {b})")?,
                ParamValue::Int(value) => write!(formatter, "{value}")?,
                ParamValue::Bool(value) => write!(formatter, "{value}")?,
            }
        }
        formatter.write_str("}")
    }
}

impl From<&str> for Binding {
    fn from(material: &str) -> Self {
        Self::new(material)
    }
}

impl From<String> for Binding {
    fn from(material: String) -> Self {
        Self::new(material)
    }
}

impl Serialize for Binding {
    /// The key alone where nothing is overridden, and a struct where something
    /// is, which is what makes this backward compatible in both directions: a
    /// recipe that overrides nothing is byte for byte the recipe it was.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        if self.params.is_empty() {
            return serializer.serialize_str(&self.material);
        }
        let mut binding = serializer.serialize_struct("Binding", 2)?;
        binding.serialize_field("material", &self.material)?;
        binding.serialize_field("params", &self.params)?;
        binding.end()
    }
}

impl<'de> Deserialize<'de> for Binding {
    /// Either form, told apart by what the document holds rather than by a tag.
    ///
    /// Hand-written rather than `#[serde(untagged)]` because untagged buffers
    /// the whole value and answers "data did not match any variant" for every
    /// mistake inside it, and a misspelled field in a hand-edited recipe should
    /// name the field.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(BindingVisitor)
    }
}

/// The visitor behind [`Binding`]'s two forms.
struct BindingVisitor;

impl<'de> serde::de::Visitor<'de> for BindingVisitor {
    type Value = Binding;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a material key, or a binding of a key and its parameter overrides")
    }

    fn visit_str<E: serde::de::Error>(self, material: &str) -> Result<Self::Value, E> {
        Ok(Binding::new(material))
    }

    fn visit_string<E: serde::de::Error>(self, material: String) -> Result<Self::Value, E> {
        Ok(Binding::new(material))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        use serde::de::Error;

        let mut material: Option<String> = None;
        let mut params: Option<BTreeMap<String, ParamValue>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "material" => {
                    if material.is_some() {
                        return Err(A::Error::duplicate_field("material"));
                    }
                    material = Some(map.next_value()?);
                }
                "params" => {
                    if params.is_some() {
                        return Err(A::Error::duplicate_field("params"));
                    }
                    params = Some(map.next_value()?);
                }
                other => {
                    return Err(A::Error::unknown_field(other, &["material", "params"]));
                }
            }
        }
        Ok(Binding {
            material: material.ok_or_else(|| A::Error::missing_field("material"))?,
            params: params.unwrap_or_default(),
        })
    }
}

/// A graph, its parameter values and a resolution: everything a bake needs.
///
/// The same request produces the shipped files and the images a
/// [`Surface::Graph`] definition creates at registration, so a texture set can
/// move between disk and memory without changing how it looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bake {
    /// Content key of the graph in a material graph library.
    pub graph: String,
    /// Values for the graph's exposed parameters; absent names keep their defaults.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// Edge length of the baked maps in texels, a power of two in 256..=4096.
    pub resolution: u32,
}

/// Where a material's varying detail comes from: nowhere, files, a graph, or a
/// shader.
///
/// Constants live on [`MaterialDefinition`] in every case; this only says how
/// the per-texel part is produced. Moving a surface between files, a graph and
/// a live shader is a change of variant here and nothing in a recipe.
///
/// A game ships [`Plain`](Self::Plain) and [`Files`](Self::Files): the content
/// step (`ashlar-content`) turns every [`Graph`](Self::Graph) into `Files`, and
/// the default-feature `ashlar-bevy` draws nothing else. `Graph` and
/// [`Shader`](Self::Shader) name a material graph, so drawing one needs the
/// graph engine a tool links.
///
/// `deny_unknown_fields` is a container attribute that serde applies to the
/// struct variants below, so a misspelled map key in a hand-edited library is a
/// parse error naming the field rather than a surface that silently names no
/// maps at all: the strictness the three flat keys carried before.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Surface {
    /// Constants only: glass, lights and trim need no maps.
    #[default]
    Plain,
    /// Texture files on disk, painted by hand or baked by the content step.
    /// Keys are paths under the asset root.
    Files {
        /// sRGB base colour texture, multiplied by [`MaterialDefinition::base_color`].
        #[serde(default)]
        base_color: Option<String>,
        /// Linear tangent-space normal map. Its green channel is read in the
        /// bake's convention, which points down a rising image row, exactly
        /// when `baked_from` is present, and as OpenGL's +Y when it is not. So
        /// a map an export wrote keeps its `baked_from`, and a painted map
        /// leaves it out.
        #[serde(default)]
        normal: Option<String>,
        /// Linear packed occlusion, roughness and metallic texture.
        #[serde(default)]
        orm: Option<String>,
        /// Linear height field in 0..=1, the same one the normal was derived from.
        #[serde(default)]
        height: Option<String>,
        /// Linear emitted RGB, multiplied by [`MaterialDefinition::emissive`].
        /// That constant defaults to zero, so a surface naming this map and
        /// leaving the constant alone emits nothing.
        #[serde(default)]
        emissive: Option<String>,
        /// Present when a bake wrote these files, so the step can be re-run and
        /// a reviewer can see where a file came from. It is also what says the
        /// normal map is in the bake's convention, so dropping it from an
        /// exported library inverts the normal map's green channel.
        #[serde(default)]
        baked_from: Option<Bake>,
    },
    /// A material graph, baked into images in memory when the material is
    /// registered, never touching disk.
    ///
    /// The same textures the content step writes for the same [`Bake`], so a
    /// `Graph` surface in a tool and its `Files` twin in a game look the same.
    /// It is how a library is authored, and the tool's variety path: a
    /// per-building seed or tint costs no files.
    Graph(Bake),
    /// A material graph compiled into a shader when the material is registered.
    ///
    /// What a texture cannot have: parameters that change per frame, time,
    /// world position, or the faces a cutter made.
    Shader {
        /// Content key of the graph in a material graph library.
        graph: String,
        /// Values for the graph's exposed parameters; absent names keep their defaults.
        #[serde(default)]
        params: BTreeMap<String, ParamValue>,
    },
}

/// Geometry scattered from the same material graph the surface is made of.
///
/// A material graph may declare *strand layers* beside its PBR output — grass,
/// fur, moss fibre, carpet pile — and this is a definition asking for them. It
/// names layers rather than describing them, for the same reason a
/// [`Surface::Graph`] names a graph: what a blade is made of belongs to the
/// graph, and what a *surface* decides is whether to grow any and how far away
/// to stop.
///
/// It is an [`Option`] on [`MaterialDefinition`] and defaults to nothing, so
/// every library written before strands existed reads and writes back
/// unchanged. This crate never learns what a layer is: the names are checked
/// against the graph by the adapter that holds both libraries, exactly as an
/// instance's parameter overrides are.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StrandSettings {
    /// Which of the graph's strand layers to build, by the names it declares
    /// them under.
    ///
    /// A layer the graph does not declare is a startup error naming the
    /// material, not a surface that quietly grows nothing.
    pub layers: Vec<String>,
    /// Where each level of detail gives way to the next, in metres from the
    /// camera, nearest first.
    ///
    /// Stored here and read by the renderer: the geometry of a level is built
    /// from the same set whatever the distances say, so changing one is a
    /// visibility range and not a rebuild. An empty list is one level that is
    /// always drawn.
    pub lod_metres: Vec<f32>,
    /// What fraction of each layer's strands to grow, in `0..=1`.
    ///
    /// The level-of-detail cut applied at full detail: a lawn authored at
    /// sixteen thousand blades a square metre is the same lawn at a quarter of
    /// them, and this is where a scene that cannot afford the first says so.
    /// It keeps a *prefix* of the set rather than thinning it randomly, so
    /// lowering it removes blades and never moves the ones that stay.
    pub density: f32,
    /// Whether the strands cast shadows.
    ///
    /// On by default, and worth turning off on a dense layer: a blade is
    /// thinner than a shadow texel, so what a lawn contributes to a shadow map
    /// is mostly aliasing, and the layer's own root occlusion is what actually
    /// puts the dark at the bottom of it.
    pub cast_shadows: bool,
    /// Where the last band of real strands gives way to *cards*, in metres.
    ///
    /// A card is a picture of a tuft on a crossed quad, and this is how far out
    /// they are drawn to. It has to be past the last
    /// [`Self::lod_metres`] entry, because a card is the level after the
    /// thinnest strands rather than a level beside them, and there has to be
    /// such an entry for it to take over from; validation says both by path.
    ///
    /// `None` is what a library written before cards existed carries: past the
    /// last distance there is no geometry at all, and what the camera sees is
    /// the relief the same strands were splatted into. That is still what
    /// happens past this distance; what this buys is the band in between, where
    /// a tuft is worth a quad and no longer worth a hundred blades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_metres: Option<f32>,
    /// The baked set these layers are grown from, as a content key an IO
    /// adapter resolves.
    ///
    /// What [`Surface::Files`] is to a wall, this is to a lawn: one file,
    /// written by a content step, holding every layer [`Self::layers`] names,
    /// so that a game grows grass without carrying the graph engine that
    /// scattered it. One key rather than one per layer, because the layers of
    /// one definition are grown together and shipped together — the file's own
    /// directory is what finds a layer inside it.
    ///
    /// `None` is what a library written before this field existed carries, and
    /// what a definition grown by a *tool* carries: the preview and the content
    /// steps hold the graph library, so they scatter the set rather than
    /// reading one. A game has neither, so a definition it draws names this and
    /// the adapter opens it before the window does.
    ///
    /// Skipped when absent, which is what keeps every library written before it
    /// byte for byte the library it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baked_set: Option<String>,
}

impl Default for StrandSettings {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            lod_metres: Vec::new(),
            density: 1.0,
            cast_shadows: true,
            card_metres: None,
            baked_set: None,
        }
    }
}

impl StrandSettings {
    /// Grow the named layers, at full density and with shadows on.
    pub fn new<I, S>(layers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            layers: layers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Set where each level of detail gives way to the next, nearest first.
    #[must_use]
    pub fn lod_metres(mut self, metres: impl IntoIterator<Item = f32>) -> Self {
        self.lod_metres = metres.into_iter().collect();
        self
    }

    /// Set what fraction of each layer to grow.
    #[must_use]
    pub fn density(mut self, density: f32) -> Self {
        self.density = density;
        self
    }

    /// Set whether the strands cast shadows.
    #[must_use]
    pub fn cast_shadows(mut self, cast: bool) -> Self {
        self.cast_shadows = cast;
        self
    }

    /// Set how far out cards are drawn, past the last level-of-detail distance.
    #[must_use]
    pub fn card_metres(mut self, metres: f32) -> Self {
        self.card_metres = Some(metres);
        self
    }

    /// Name the baked set file these layers are grown from.
    #[must_use]
    pub fn baked_set(mut self, key: impl Into<String>) -> Self {
        self.baked_set = Some(key.into());
        self
    }

    /// Check the layer names, the density and the level-of-detail distances.
    ///
    /// What this cannot check is whether the graph declares those layers, which
    /// is the adapter's half: a graph lives in a crate beside this one.
    pub fn check(&self, path: &str) -> Result<(), ValidationError> {
        require(
            !self.layers.is_empty(),
            &format!("{path}.layers"),
            "a strand setting that names no layer grows nothing; leave `strands` out instead",
        )?;
        for (index, layer) in self.layers.iter().enumerate() {
            name(layer, &format!("{path}.layers[{index}]"))?;
            require(
                !self.layers[..index].contains(layer),
                &format!("{path}.layers[{index}]"),
                &format!("layer {layer:?} is named twice, and would be grown twice"),
            )?;
        }
        require(
            self.density.is_finite() && (0.0..=1.0).contains(&self.density),
            &format!("{path}.density"),
            "density must be finite in 0..=1",
        )?;
        let mut previous = 0.0_f32;
        for (index, metres) in self.lod_metres.iter().enumerate() {
            require(
                metres.is_finite() && *metres > previous,
                &format!("{path}.lod_metres[{index}]"),
                "level-of-detail distances must be finite and increase away from the camera",
            )?;
            previous = *metres;
        }
        if let Some(cards) = self.card_metres {
            // The same rule the distances above are held to, continued: a card
            // is the level *after* the thinnest strands, so a card distance
            // inside the strand bands would draw a picture of a tuft in front
            // of the tuft it is a picture of.
            require(
                cards.is_finite() && cards > previous,
                &format!("{path}.card_metres"),
                "a card distance is where the last band of strands gives way, so it must be \
                 finite and further than every level-of-detail distance",
            )?;
            // And there has to be a band for it to give way from. A layer that
            // names no distance is drawn at full detail however far away it is,
            // so a card level behind it would stand a picture of a tuft inside
            // the tuft itself.
            require(
                !self.lod_metres.is_empty(),
                &format!("{path}.card_metres"),
                "cards take over from the last level of detail, so a layer that asks for them \
                 has to name at least one `lod_metres` distance",
            )?;
        }
        // The same rule every other content key in this file is held to, and
        // the same reason: a key is resolved by an adapter against an asset
        // root, so a blank one or one with a separator in the wrong place is a
        // startup failure with nothing in it to search for.
        if let Some(key) = &self.baked_set {
            name(key, &format!("{path}.baked_set"))?;
        }
        Ok(())
    }
}

impl Surface {
    /// Whether this surface names a material graph a strand layer could be
    /// scattered from.
    ///
    /// `Graph` and `Shader` name one outright. A `Files` surface names one
    /// through `baked_from`, which is provenance rather than a dependency for
    /// its *texels* — the files it names are what load — but is the graph a
    /// strand layer belongs to all the same, and the only way a shipped library
    /// can grow one.
    pub fn graph(&self) -> Option<&str> {
        match self {
            Self::Shader { graph, .. } => Some(graph),
            Self::Graph(bake)
            | Self::Files {
                baked_from: Some(bake),
                ..
            } => Some(&bake.graph),
            Self::Plain | Self::Files { .. } => None,
        }
    }

    /// The five maps paired with the field each is authored under, or `None`
    /// when this surface names no files.
    ///
    /// Destructured exhaustively on purpose: a sixth map added to the variant
    /// stops compiling here rather than going missing from preflight,
    /// validation and [`Surface::texture_keys`] in silence.
    fn maps(&self) -> Option<[(&'static str, Option<&str>); 5]> {
        match self {
            Self::Files {
                base_color,
                normal,
                orm,
                height,
                emissive,
                baked_from: _,
            } => Some([
                ("base_color", base_color.as_deref()),
                ("normal", normal.as_deref()),
                ("orm", orm.as_deref()),
                ("height", height.as_deref()),
                ("emissive", emissive.as_deref()),
            ]),
            Self::Plain | Self::Graph(_) | Self::Shader { .. } => None,
        }
    }

    /// Texture keys for preflight loading; empty unless this is [`Surface::Files`].
    ///
    /// No filesystem assumptions in the domain: these are the keys an adapter
    /// resolves, in the order base colour, normal, ORM, height, emissive.
    pub fn texture_keys(&self) -> impl Iterator<Item = &str> {
        self.maps().into_iter().flatten().filter_map(|(_, key)| key)
    }

    /// The graph parameters this surface binds, where it names a graph.
    ///
    /// `None` for `Plain` and `Files`, and for `Files` that is a
    /// decision rather than an omission: its `baked_from` is provenance rather
    /// than a request — the files it names are what load — so there is nothing
    /// there for an instance to override.
    pub fn params(&self) -> Option<&BTreeMap<String, ParamValue>> {
        match self {
            Self::Graph(bake) => Some(&bake.params),
            Self::Shader { params, .. } => Some(params),
            Self::Plain | Self::Files { .. } => None,
        }
    }

    /// This surface with `overrides` written over the parameter values it binds.
    ///
    /// The same graph, the same resolution and the same everything else: an
    /// override names a value and never a graph, which is what keeps a
    /// per-instance override one material key rather than a second definition.
    /// `None` where this surface names no graph, which a caller reports by the
    /// path of the binding that asked.
    pub fn with_params(&self, overrides: &BTreeMap<String, ParamValue>) -> Option<Self> {
        let merged = |params: &BTreeMap<String, ParamValue>| {
            let mut params = params.clone();
            params.extend(overrides.iter().map(|(name, value)| (name.clone(), *value)));
            params
        };
        match self {
            Self::Graph(bake) => Some(Self::Graph(Bake {
                params: merged(&bake.params),
                ..bake.clone()
            })),
            Self::Shader { graph, params } => Some(Self::Shader {
                graph: graph.clone(),
                params: merged(params),
            }),
            Self::Plain | Self::Files { .. } => None,
        }
    }

    /// What this surface is called in a message about it.
    fn label(&self) -> &'static str {
        match self {
            Self::Plain => "Plain",
            Self::Files { .. } => "Files",
            Self::Graph(_) => "Graph",
            Self::Shader { .. } => "Shader",
        }
    }

    /// Check the keys, graph names, parameters and bake resolution this surface names.
    pub fn check(&self, path: &str) -> Result<(), ValidationError> {
        for (field, key) in self.maps().into_iter().flatten() {
            if let Some(key) = key {
                name(key, &format!("{path}.{field}"))?;
            }
        }
        match self {
            Self::Plain => Ok(()),
            Self::Files { baked_from, .. } => match baked_from {
                Some(bake) => check_bake(bake, &format!("{path}.baked_from")),
                None => Ok(()),
            },
            Self::Graph(bake) => check_bake(bake, path),
            Self::Shader { graph, params } => check_graph(graph, params, path),
        }
    }
}

/// A bake is a graph request plus a resolution the bake pipeline can rasterise.
fn check_bake(bake: &Bake, path: &str) -> Result<(), ValidationError> {
    check_graph(&bake.graph, &bake.params, path)?;
    require(
        bake.resolution.is_power_of_two() && (256..=4096).contains(&bake.resolution),
        &format!("{path}.resolution"),
        "bake resolution must be a power of two in 256..=4096",
    )
}

/// The half a shader surface shares with a bake: a named graph and finite parameters.
fn check_graph(
    graph: &str,
    params: &BTreeMap<String, ParamValue>,
    path: &str,
) -> Result<(), ValidationError> {
    name(graph, &format!("{path}.graph"))?;
    for (key, value) in params {
        // The key is in the path even when it is the key that is wrong: a
        // blank name is impossible to find in a large map without it.
        name(key, &format!("{path}.params[{key}]"))?;
        require(
            value.is_finite(),
            &format!("{path}.params[{key}]"),
            "parameter value must be finite",
        )?;
    }
    Ok(())
}

/// Opaque PBR surface definition. Paths are content keys resolved by an IO adapter.
/// Linear data maps use OpenGL normals and packed R=occlusion/G=roughness/B=metallic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MaterialDefinition {
    /// sRGB color multiplier; opaque RGB only in this initial material contract.
    pub base_color: [f32; 3],
    /// Perceptual roughness multiplier, from zero to one.
    pub roughness: f32,
    /// Metallic multiplier, from zero to one.
    pub metallic: f32,
    /// Linear emitted RGB, allowed to exceed one.
    pub emissive: [f32; 3],
    /// Physical size of one texture repeat along the two projected UV axes.
    pub tile_metres: [f32; 2],
    /// This definition lays its graph at this many times the repeat the graph
    /// was drawn for, on purpose.
    ///
    /// Advisory, like `tile_metres` itself: nothing here changes what a
    /// backend rasterises or binds, only what a tiling check is quiet about.
    /// A specimen sheet or a benchmark scales a whole material down to fake
    /// distance, and a storey that would not otherwise divide evenly into a
    /// material's declared repeat is sized to a whole number of repeats
    /// instead — both lay the graph's own repeat at `tile_scale` times itself
    /// on purpose, and this is how a definition says the disagreement between
    /// `tile_metres` and the graph's declared one is deliberate rather than a
    /// typo. Skipped when absent, which is what keeps a library written
    /// before this field existed byte for byte the library it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_scale: Option<f32>,
    /// Pattern offset in texture repeats, useful for instance-bound finish variants.
    pub uv_offset: [f32; 2],
    /// Where the varying detail comes from, if there is any.
    pub surface: Surface,
    /// Geometry grown out of the same graph, where the surface names one.
    ///
    /// Skipped when absent, which is what keeps every library written before
    /// strands existed byte for byte the library it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strands: Option<StrandSettings>,
}

impl Default for MaterialDefinition {
    fn default() -> Self {
        Self {
            base_color: [1.0; 3],
            roughness: 0.8,
            metallic: 0.0,
            emissive: [0.0; 3],
            tile_metres: [1.0; 2],
            tile_scale: None,
            uv_offset: [0.0; 2],
            surface: Surface::Plain,
            strands: None,
        }
    }
}

impl MaterialDefinition {
    /// Check numeric ranges and resource identities before creating GPU assets.
    pub fn check(&self, path: &str) -> Result<(), ValidationError> {
        require(
            self.uv_offset.iter().all(|v| v.is_finite()),
            path,
            "UV offset must be finite",
        )?;
        require(
            self.base_color
                .iter()
                .chain([&self.roughness, &self.metallic])
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            path,
            "color, roughness and metallic must be finite in 0..=1",
        )?;
        require(
            self.emissive.iter().all(|v| v.is_finite() && *v >= 0.0),
            path,
            "emissive must be finite and nonnegative",
        )?;
        require(
            self.tile_metres
                .iter()
                .all(|v| v.is_finite() && *v > 0.0 && v.recip().is_finite()),
            path,
            "texture repeat size must be finite and positive with a finite reciprocal",
        )?;
        require(
            self.tile_scale.is_none_or(|v| v.is_finite() && v > 0.0),
            path,
            "declared tile scale must be finite and positive",
        )?;
        self.surface.check(&format!("{path}.surface"))?;
        let Some(strands) = &self.strands else {
            return Ok(());
        };
        // A strand layer has to come from *somewhere*, and since 2026-09-20
        // there are two somewheres. A graph the surface names is the tool's:
        // `Graph` and `Shader` name one outright and `Files` names one
        // through its `baked_from`, and a scatter reads the layer out of it. A
        // `baked_set` is the game's: the layers are already grown and are in a
        // file, and the surface may then be anything at all — a lawn over
        // painted textures, or over nothing but constants.
        require(
            self.surface.graph().is_some() || strands.baked_set.is_some(),
            &format!("{path}.strands"),
            &format!(
                "this material has a {} surface, which names no material graph, and its strand \
                 settings name no `baked_set`; a strand layer is either scattered from a graph \
                 or read from a file, so give it a Graph or Shader surface, a Files surface \
                 that records the bake it came from, or the baked set it grows from",
                self.surface.label()
            ),
        )?;
        strands.check(&format!("{path}.strands"))
    }

    /// Resource keys for preflight loading; no filesystem assumptions in the domain.
    pub fn texture_keys(&self) -> impl Iterator<Item = &str> {
        self.surface.texture_keys()
    }

    /// The baked strand set this definition grows from, where it names one.
    ///
    /// Beside [`Self::texture_keys`] rather than in it: both are keys an
    /// adapter resolves and opens before a window exists, and they are not the
    /// same kind of file — a map is a texture container and this is a list of
    /// blades — so an adapter that opened one as the other would report the
    /// wrong thing about it.
    pub fn strand_set_key(&self) -> Option<&str> {
        self.strands.as_ref()?.baked_set.as_deref()
    }

    /// This definition as a [`Binding`] dresses it.
    ///
    /// Itself where the binding overrides nothing, which is nearly every slot
    /// in nearly every recipe, and a copy with the overrides written into its
    /// surface otherwise — so a caller applies a binding once and then deals in
    /// a definition, with nothing below it knowing an override happened.
    /// `None` where the binding overrides parameters of a surface that binds
    /// none; [`MaterialLibrary::check_for`] has already refused that by path,
    /// so a caller past validation can treat it as unreachable.
    pub fn overridden<'a>(&'a self, binding: &Binding) -> Option<Cow<'a, Self>> {
        if binding.params.is_empty() {
            return Some(Cow::Borrowed(self));
        }
        Some(Cow::Owned(Self {
            surface: self.surface.with_params(&binding.params)?,
            ..self.clone()
        }))
    }
}

/// What a [`MaterialLibrary`] is checked against: every slot something binds.
///
/// A building lives in a crate above this one, so the library asks for the one
/// thing it needs of it rather than for the building. `ashlar::Building`
/// implements this, which is what keeps `library.check_for(&building)` the
/// call it always was.
pub trait BoundSlots {
    /// Every binding in use, each with the path an error about it is reported
    /// at, such as `instances[tower].materials[stone]`.
    fn bound_slots(&self) -> impl Iterator<Item = (String, &Binding)>;
}

/// Independently authored material library, allowing styles to share recipes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialLibrary {
    /// Content key to surface definition. Recipe slots bind these keys.
    pub materials: BTreeMap<String, MaterialDefinition>,
}

impl MaterialLibrary {
    /// Validate definitions and all material keys used by a building, including cuts.
    ///
    /// A binding that overrides graph parameters is checked twice over: the
    /// definition it names has to exist, as every binding's does, and its
    /// surface has to be one that binds parameters at all — a
    /// [`Surface::Graph`] or a [`Surface::Shader`]. What this cannot check is
    /// whether the names it sets are parameters the *graph* declares, because a
    /// graph lives in a crate beside this one; the adapter that reads both
    /// libraries checks that, by the same path.
    pub fn check_for(&self, building: &impl BoundSlots) -> Result<(), ValidationError> {
        for (id, material) in &self.materials {
            name(id, "materials")?;
            material.check(&format!("materials[{id}]"))?;
        }
        for (path, binding) in building.bound_slots() {
            let Some(definition) = self.materials.get(&binding.material) else {
                return Err(ValidationError {
                    path,
                    reason: format!("missing material definition {:?}", binding.material),
                });
            };
            require(
                binding.params.is_empty() || definition.surface.params().is_some(),
                &path,
                &format!(
                    "material {:?} has a {} surface, which binds no graph parameters; an \
                     instance may override the parameters of a Graph or Shader surface only",
                    binding.material,
                    definition.surface.label()
                ),
            )?;
        }
        Ok(())
    }
}
