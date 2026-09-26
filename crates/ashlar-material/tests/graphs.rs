//! Authoring, validation and interchange, independent of any backend.
//!
//! Every case that fails asserts the path it failed at: a path is the whole
//! point of the error type, and a validator that says "invalid graph" would
//! pass a test that only checked `is_err`.
use ashlar_material::{
    BlendMode, GraphError, GraphParam, Input, Material, MaterialGraph, MaterialGraphLibrary,
    MathOp, Node, Param, ParamValue, PbrOutput, Period, Value, ValueType,
    nodes::{
        Blend, Bricks, Colorize, Combine, Comment, Decompose, GraphInput, Invert, Levels, Math,
        Mirror, Noise, Subgraph, SurfaceOutput, TilePattern, Tiles, Transform, Uv, Warp,
    },
};

/// The smallest graph that validates: one noise reaching every output.
fn graph() -> MaterialGraph {
    MaterialGraph::builder("test:plain")
        .node("noise", Noise::value().period(8))
        .output(
            PbrOutput::new()
                .base_color("noise")
                .roughness("noise")
                .height("noise")
                .normal_strength(0.01),
        )
        .into_graph()
}

/// The path of the error a graph fails with, for the table-driven cases.
fn rejects(graph: MaterialGraph) -> GraphError {
    graph.build().expect_err("expected a rejection")
}

#[test]
fn a_built_material_carries_the_type_and_period_of_every_port() {
    let material = graph().build().expect("valid");
    let port = material.port("noise").expect("inferred");
    assert_eq!(port.value_type, ValueType::Float);
    assert_eq!(port.period, Period::square(8));
    assert_eq!(material.period(), Period::square(8));
    // The base colour took a float and broadcast it, so the port stays a float
    // and the conversion happens where the value is read, not where it is made.
    assert_eq!(
        material.output_port("base_color").map(|p| p.value_type),
        Some(ValueType::Float)
    );
    assert_eq!(material.output_port("emissive"), None);
    assert_eq!(material.order(), ["noise"]);
    assert!(material.warnings().is_empty());
}

#[test]
fn unknown_nodes_parameters_and_self_references_are_reported_by_input_path() {
    let unknown_node = MaterialGraph::builder("test:graph")
        .node("blend", Blend::new(BlendMode::Add, "missing", 0.5))
        .output(PbrOutput::new().base_color("blend"))
        .into_graph();
    let error = rejects(unknown_node);
    assert_eq!(error.path, "nodes[blend].inputs[a]");
    assert!(error.reason.contains("unknown node"), "{error}");

    let unknown_param = MaterialGraph::builder("test:graph")
        .node(
            "blend",
            Blend::new(BlendMode::Add, Input::param("wear"), 0.5),
        )
        .output(PbrOutput::new().base_color("blend"))
        .into_graph();
    let error = rejects(unknown_param);
    assert_eq!(error.path, "nodes[blend].inputs[a]");
    assert!(error.reason.contains("unknown parameter"), "{error}");

    let itself = MaterialGraph::builder("test:graph")
        .node("levels", Levels::new("levels"))
        .output(PbrOutput::new().base_color("levels"))
        .into_graph();
    let error = rejects(itself);
    assert_eq!(error.path, "nodes[levels].inputs[input]");
    assert_eq!(error.reason, "node references itself");

    // The output is validated by the same rules, under its own path.
    let mut unknown_output = graph();
    unknown_output.output.roughness = Input::node("absent");
    let error = rejects(unknown_output);
    assert_eq!(error.path, "output.roughness");
}

#[test]
fn an_operand_a_unary_operator_never_reads_does_not_widen_its_answer() {
    // `b` is a port on every `Math`, so a hand-written or deserialized graph
    // may wire a colour into one an operator never reads. The answer is one
    // channel either way, and the inferred type has to be the one the lowering
    // will produce rather than the widest thing wired.
    let material = MaterialGraph::builder("test:graph")
        .node("mask", Noise::value().period(4))
        .node("tint", Combine::new(0.2, 0.4, 0.6))
        .node("unary", Math::new(MathOp::Abs, "mask", "tint"))
        .node("binary", Math::new(MathOp::Mul, "mask", "tint"))
        .output(PbrOutput::new().base_color("binary").roughness("unary"))
        .build()
        .expect("valid");
    assert_eq!(
        material.port("unary").map(|port| port.value_type),
        Some(ValueType::Float)
    );
    assert_eq!(
        material.port("binary").map(|port| port.value_type),
        Some(ValueType::Color)
    );
}

#[test]
fn a_cycle_is_reported_at_the_input_that_closes_it() {
    let cyclic = MaterialGraph::builder("test:graph")
        .node("a", Invert::new("c"))
        .node("b", Invert::new("a"))
        .node("c", Invert::new("b"))
        .output(PbrOutput::new().base_color("a"))
        .into_graph();
    let error = rejects(cyclic);
    // Whichever node the walk reaches last is the one that closes the loop,
    // and the reason names the whole ring.
    assert!(
        error.path.starts_with("nodes[") && error.path.ends_with("].inputs[input]"),
        "{error}"
    );
    assert!(error.reason.starts_with("cycle: "), "{error}");
    for id in ["a", "b", "c"] {
        assert!(error.reason.contains(id), "{error}");
    }
}

#[test]
fn a_vec2_converts_to_nothing_and_says_so_where_it_arrived() {
    let into_float = MaterialGraph::builder("test:graph")
        .node("uv", Uv::new())
        .node("levels", Levels::new("uv"))
        .output(PbrOutput::new().base_color("levels"))
        .into_graph();
    let error = rejects(into_float);
    assert_eq!(error.path, "nodes[levels].inputs[input]");
    assert!(
        error.reason.contains("Vec2, which converts to nothing"),
        "{error}"
    );

    let mut into_output = graph();
    into_output.nodes.insert("uv".into(), Uv::new().into());
    into_output.output.base_color = Input::node("uv");
    assert_eq!(rejects(into_output).path, "output.base_color");

    // A float and a colour do convert, in both directions.
    let converted = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value().period(2))
        .node("rgb", Colorize::new("noise"))
        .node("back", Levels::new("rgb"))
        .output(PbrOutput::new().base_color("back").roughness("rgb"))
        .build()
        .expect("float broadcasts and colour takes its luminance");
    assert_eq!(
        converted.port("back").map(|p| p.value_type),
        Some(ValueType::Color)
    );
}

#[test]
fn a_vec2_reaches_a_float_only_by_being_decomposed() {
    let material = MaterialGraph::builder("test:graph")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", ashlar_material::Channel::R))
        .output(PbrOutput::new().base_color("u").roughness("u"))
        .build()
        .expect("a decomposed UV is a float");
    assert_eq!(
        material.port("u").map(|p| p.value_type),
        Some(ValueType::Float)
    );

    let no_blue = MaterialGraph::builder("test:graph")
        .node("uv", Uv::new())
        .node("b", Decompose::new("uv", ashlar_material::Channel::B))
        .output(PbrOutput::new().base_color("b"))
        .into_graph();
    let error = rejects(no_blue);
    assert_eq!(error.path, "nodes[b].channel");
}

#[test]
fn a_comment_has_no_output_and_nothing_may_read_it() {
    let reading_a_comment = MaterialGraph::builder("test:graph")
        .node("note", Comment::new("mind the seam"))
        .node("levels", Levels::new("note"))
        .output(PbrOutput::new().base_color("levels"))
        .into_graph();
    let error = rejects(reading_a_comment);
    assert_eq!(error.path, "nodes[levels].inputs[input]");
    assert!(error.reason.contains("has no output"), "{error}");

    // On its own it is simply ignored, and still appears in the order.
    let material = MaterialGraph::builder("test:graph")
        .node("note", Comment::new("mind the seam"))
        .node("noise", Noise::value().period(2))
        .output(PbrOutput::new().base_color("noise"))
        .build()
        .expect("a comment is data an author reads");
    assert!(material.order().contains(&"note".to_owned()));
    assert_eq!(material.port("note"), None);
}

#[test]
fn names_duplicates_and_node_fields_are_checked_by_path() {
    let mut blank = graph();
    blank.id = "  ".into();
    assert_eq!(rejects(blank).path, "id");

    let mut blank_node = graph();
    blank_node.nodes.insert(String::new(), Uv::new().into());
    assert_eq!(rejects(blank_node).path, "nodes[]");

    let duplicate_param = MaterialGraph::builder("test:graph")
        .param(Param::float("wear", 0.0))
        .param(Param::color("wear", [1.0; 3]))
        .node("noise", Noise::value())
        .output(PbrOutput::new().base_color("noise"))
        .into_graph();
    let error = rejects(duplicate_param);
    assert_eq!(error.path, "params[wear]");
    assert_eq!(error.reason, "duplicate identity");

    // A duplicate node id is a map overwrite, so the builder remembers instead.
    let error = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value())
        .node("noise", Noise::perlin())
        .output(PbrOutput::new().base_color("noise"))
        .build()
        .expect_err("duplicate node id");
    assert_eq!(error.path, "nodes[noise]");

    for (node, path) in [
        (Node::from(Noise::value().period(0)), "nodes[bad].period"),
        (
            Noise::value().period(64).octaves(9).lacunarity(4).into(),
            "nodes[bad]",
        ),
        (Levels::new("noise").gamma(0.0).into(), "nodes[bad].gamma"),
        (
            Levels::new("noise").in_range(0.5, 0.5).into(),
            "nodes[bad].in_high",
        ),
        (Bricks::new().rows(0).into(), "nodes[bad].rows"),
        (Bricks::new().mortar(2.0).into(), "nodes[bad].mortar"),
        // A half is taken off both sides of a one-unit brick, which leaves no
        // face: a blank mask is a path rather than a black texture.
        (Bricks::new().mortar(0.5).into(), "nodes[bad].mortar"),
        // A bond that does not divide the rows never meets itself in v.
        (Bricks::new().rows(3).into(), "nodes[bad].offset"),
        (
            Tiles::new().pattern(TilePattern::Hex).rows(3).into(),
            "nodes[bad].rows",
        ),
        (
            Colorize::new("noise")
                .gradient([(1.0, [0.0; 3]), (0.0, [1.0; 3])])
                .into(),
            "nodes[bad].gradient",
        ),
        (
            Transform::new("noise").scale(f32::NAN).into(),
            "nodes[bad].scale",
        ),
        (GraphInput::float("  ", 0.5).into(), "nodes[bad].name"),
        // A default is what the input carries until something binds it, so it
        // has to be a value of the type the input presents, and one a texel
        // survives. Written field by field rather than through a builder,
        // because a builder cannot make either of these.
        (
            GraphInput {
                name: "wear".to_owned(),
                value_type: ValueType::Float,
                default: Value::Color([0.5; 3]),
            }
            .into(),
            "nodes[bad].default",
        ),
        (
            GraphInput::float("wear", f32::NAN).into(),
            "nodes[bad].default",
        ),
    ] {
        let graph = MaterialGraph::builder("test:graph")
            .node("noise", Noise::value())
            .node("bad", node)
            .output(PbrOutput::new().base_color("noise"))
            .into_graph();
        assert_eq!(rejects(graph).path, path);
    }
}

#[test]
fn a_parameter_is_named_and_carries_a_value_a_texel_survives() {
    // A blank name is the same rule a part id follows, and it is checked on
    // the parameter rather than where something refers to it: a reference to
    // "" would otherwise read as an unknown parameter.
    let blank = MaterialGraph::builder("test:graph")
        .param(Param::float("   ", 0.5))
        .node("noise", Noise::value())
        .output(PbrOutput::new().base_color("noise"))
        .into_graph();
    let error = rejects(blank);
    assert_eq!(error.path, "params[   ].name");
    assert!(error.reason.contains("blank"), "{error}");

    // A NaN default poisons every texel it reaches, and a colour carries three
    // chances to do it.
    for param in [
        Param::float("wear", f32::NAN),
        Param::float("wear", f32::INFINITY),
        Param::color("wear", [0.5, f32::NAN, 0.5]),
    ] {
        let graph = MaterialGraph::builder("test:graph")
            .param(param)
            .node("noise", Noise::value())
            .output(PbrOutput::new().base_color("noise"))
            .into_graph();
        assert_eq!(rejects(graph).path, "params[wear].value");
    }
}

#[test]
fn a_parameter_range_holds_its_own_default_and_belongs_to_a_number() {
    for param in [
        Param::float("wear", 2.0).range(0.0, 1.0),
        Param::float("wear", 0.5).range(1.0, 0.0),
        Param::color("tint", [0.5; 3]).range(0.0, 1.0),
    ] {
        let name = param.name.clone();
        let graph = MaterialGraph::builder("test:graph")
            .param(param)
            .node("noise", Noise::value())
            .output(PbrOutput::new().base_color("noise"))
            .into_graph();
        assert_eq!(rejects(graph).path, format!("params[{name}].range"));
    }
    let mut not_finite = graph();
    not_finite.output.roughness = Input::Const(Value::Float(f32::NAN));
    assert_eq!(rejects(not_finite).path, "output.roughness");
}

#[test]
fn an_inline_node_is_hoisted_under_the_id_of_the_input_it_was_written_in() {
    let material = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value().period(4))
        .node(
            "dirty",
            Blend::new(BlendMode::Multiply, "noise", Invert::new("noise")),
        )
        .output(
            PbrOutput::new()
                .base_color("dirty")
                .roughness(Levels::new("noise").out_range(0.8, 0.9)),
        )
        .build()
        .expect("inline nodes are hoisted, not rejected");
    assert!(material.port("dirty.b").is_some(), "an inline input");
    assert!(
        material.port("output.roughness").is_some(),
        "an inline output"
    );
    // And the graph that comes back holds plain node references, so what Serde
    // writes is what a backend reads.
    assert_eq!(
        material.graph().nodes["dirty"],
        Node::from(Blend::new(BlendMode::Multiply, "noise", "dirty.b")),
    );

    let collision = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value())
        .node("levels.input", Noise::perlin())
        .node("levels", Levels::new(Invert::new("noise")))
        .output(PbrOutput::new().base_color("levels"))
        .into_graph();
    let error = rejects(collision);
    assert_eq!(error.path, "nodes[levels.input]");
}

#[test]
fn the_builder_and_serde_write_the_same_data_and_ron_round_trips() {
    let material = MaterialGraph::builder("test:ron")
        .param(Param::float("wear", 0.25).range(0.0, 1.0).live())
        .param(Param::color("tint", [0.6, 0.5, 0.4]))
        .node("noise", Noise::perlin().period(8).octaves(3))
        .node("seams", Bricks::new().rows(4).columns(2))
        .node(
            "mix",
            Blend::new(BlendMode::Subtract, "noise", "seams").opacity(Input::param("wear")),
        )
        .node(
            "albedo",
            Colorize::new("mix").gradient([(0.0, [0.1; 3]), (1.0, [0.9; 3])]),
        )
        .node("rgb", Combine::new("mix", "mix", Input::param("wear")))
        .output(
            PbrOutput::new()
                .base_color("albedo")
                .roughness("mix")
                .emissive("rgb")
                .height("mix")
                .normal_strength(0.02),
        )
        .build()
        .expect("valid");

    let encoded = ron::to_string(material.graph()).expect("serialize");
    let decoded: MaterialGraph = ron::from_str(&encoded).expect("deserialize");
    let rebuilt = decoded.build().expect("a written graph validates again");
    assert_eq!(rebuilt, material);
    assert_eq!(rebuilt.period(), material.period());

    // Unknown fields are refused rather than silently dropped.
    let corrupted = encoded.replacen("id:", "typo:", 1);
    assert!(ron::from_str::<MaterialGraph>(&corrupted).is_err());
}

#[test]
fn a_declared_repeat_round_trips_is_checked_and_writes_nothing_when_absent() {
    // The whole point of the default: a graph that declares no repeat writes
    // exactly the text it wrote before the field existed, so every shipped
    // library and every byte-pinned golden is the one it was.
    let silent = graph().build().expect("valid");
    assert_eq!(silent.graph().tile_metres, None);
    let encoded = ron::to_string(silent.graph()).expect("serialize");
    assert!(
        !encoded.contains("tile_metres"),
        "an undeclared repeat is not written: {encoded}"
    );

    // And a declared one is data like any other: written, read back, and the
    // same graph on the other side.
    let declared = MaterialGraph::builder("test:paving")
        .node("noise", Noise::value().period(8))
        .output(PbrOutput::new().height("noise").normal_strength(0.01))
        .tile_metres([2.0, 1.5])
        .build()
        .expect("valid");
    assert_eq!(declared.graph().tile_metres, Some([2.0, 1.5]));
    let encoded = ron::to_string(declared.graph()).expect("serialize");
    let decoded: MaterialGraph = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded.build().expect("valid again"), declared);

    // It decides no texel, so what it may not be is a number the crate that
    // divides by it would choke on.
    for (metres, axis) in [
        ([0.0, 1.0], 0),
        ([1.0, -2.0], 1),
        ([f32::NAN, 1.0], 0),
        ([1.0, f32::INFINITY], 1),
    ] {
        let error = rejects(
            MaterialGraph::builder("test:paving")
                .node("noise", Noise::value().period(8))
                .output(PbrOutput::new().roughness("noise"))
                .tile_metres(metres)
                .into_graph(),
        );
        assert_eq!(error.path, format!("tile_metres[{axis}]"), "{metres:?}");
    }
}

#[test]
fn a_library_round_trips_and_resolves_a_subgraph_by_key() {
    let leaf = MaterialGraph::builder("test:leaf")
        .node("noise", Noise::value().period(16))
        .output(
            PbrOutput::new()
                .base_color("noise")
                .roughness("noise")
                .height("noise"),
        )
        .into_graph();
    let user = MaterialGraph::builder("test:user")
        .node(
            "inner",
            Subgraph::new("test:leaf").output(SurfaceOutput::Height),
        )
        .node("bumpy", Levels::new("inner").out_range(0.2, 0.8))
        .output(PbrOutput::new().base_color("bumpy"))
        .into_graph();
    let mut library = MaterialGraphLibrary::default();
    library.insert(leaf);
    library.insert(user);

    let built = library
        .build("test:user")
        .expect("resolved through the library");
    // The subgraph passes the period of the output it reads straight through.
    assert_eq!(
        built.port("inner").map(|p| p.period),
        Some(Period::square(16))
    );
    assert_eq!(built.period(), Period::square(16));
    library.check().expect("every graph validates");

    let encoded = ron::to_string(&library).expect("serialize");
    assert_eq!(
        ron::from_str::<MaterialGraphLibrary>(&encoded).expect("decode"),
        library
    );

    // Without the library the same graph cannot know what it instanced.
    let alone = library.get("test:user").expect("present").clone();
    let error = alone.build().expect_err("no library");
    assert_eq!(error.path, "nodes[inner].graph");
    assert!(error.reason.contains("unknown graph"), "{error}");
}

#[test]
fn a_graph_may_not_include_itself_directly_or_through_another() {
    let mut library = MaterialGraphLibrary::default();
    for (id, instanced) in [("test:a", "test:b"), ("test:b", "test:a")] {
        library.insert(
            MaterialGraph::builder(id)
                .node("inner", Subgraph::new(instanced))
                .output(PbrOutput::new().base_color("inner"))
                .into_graph(),
        );
    }
    let error = library.build("test:a").expect_err("recursive");
    assert_eq!(
        error.path,
        "graphs[test:a].graphs[test:b].nodes[inner].graph"
    );
    assert!(error.reason.starts_with("recursive subgraph: "), "{error}");
    assert!(
        error.reason.contains("test:a -> test:b -> test:a"),
        "{error}"
    );

    library.insert(
        MaterialGraph::builder("test:self")
            .node("inner", Subgraph::new("test:self"))
            .output(PbrOutput::new().base_color("inner"))
            .into_graph(),
    );
    let error = library.build("test:self").expect_err("recursive");
    assert_eq!(error.path, "graphs[test:self].nodes[inner].graph");
}

#[test]
fn a_subgraph_binds_only_parameters_the_graph_it_instances_has() {
    let leaf = MaterialGraph::builder("test:leaf")
        .param(Param::float("wear", 0.5))
        .param(Param::color("tint", [0.5; 3]))
        .node("noise", Noise::value().period(4))
        .node(
            "tinted",
            Blend::new(BlendMode::Multiply, "noise", Input::param("tint")),
        )
        .output(PbrOutput::new().base_color("tinted"))
        .into_graph();
    let user = |instance: Subgraph| {
        let mut library = MaterialGraphLibrary::default();
        library.insert(leaf.clone());
        library.insert(
            MaterialGraph::builder("test:user")
                .node("inner", instance)
                .output(PbrOutput::new().base_color("inner"))
                .into_graph(),
        );
        library.build("test:user")
    };
    user(Subgraph::new("test:leaf").param("wear", ParamValue::Float(0.2)))
        .expect("a name the instanced graph has, at the type it has");

    // A misspelled name would otherwise bind nothing and leave the default in
    // place, so the node would look wired and do nothing.
    let error = user(Subgraph::new("test:leaf").param("war", ParamValue::Float(0.2)))
        .expect_err("no such parameter");
    assert_eq!(error.path, "graphs[test:user].nodes[inner].params[war]");
    assert!(error.reason.contains("has no parameter"), "{error}");

    // Nor may a binding change what a parameter carries.
    let error = user(Subgraph::new("test:leaf").param("wear", ParamValue::Color([0.5; 3])))
        .expect_err("a colour is not a float");
    assert_eq!(error.path, "graphs[test:user].nodes[inner].params[wear]");
    assert!(error.reason.contains("takes a Float"), "{error}");

    // An integer or a boolean is a float, the way it is everywhere else here.
    user(Subgraph::new("test:leaf").param("wear", ParamValue::Int(1)))
        .expect("an integer widens into a float parameter");
}

#[test]
fn a_warning_raised_inside_an_instanced_graph_reaches_the_graph_that_used_it() {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:leaf")
            .node("four", Noise::value().period(4))
            .node("six", Noise::value().period(6))
            .node("mix", Blend::new(BlendMode::Add, "four", "six"))
            .output(PbrOutput::new().base_color("mix"))
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("test:user")
            .node("inner", Subgraph::new("test:leaf"))
            .output(PbrOutput::new().base_color("inner"))
            .into_graph(),
    );
    let built = library.build("test:user").expect("valid");
    let warnings = built.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    // Rooted at the key it came from, the way an error inside an instanced
    // graph is: the author who instanced it is the one who has to read it.
    assert_eq!(warnings[0].path, "graphs[test:leaf].nodes[mix]");
    assert!(warnings[0].message.contains("12x12"), "{}", warnings[0]);
    // And it is raised once, however often the graph is instanced.
    library.insert(
        MaterialGraph::builder("test:user")
            .node("inner", Subgraph::new("test:leaf"))
            .node(
                "again",
                Subgraph::new("test:leaf").output(SurfaceOutput::Roughness),
            )
            .output(PbrOutput::new().base_color("inner").roughness("again"))
            .into_graph(),
    );
    assert_eq!(
        library.build("test:user").expect("valid").warnings().len(),
        1
    );
}

#[test]
fn reading_an_output_a_subgraph_does_not_bind_is_an_error_by_path() {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:flat")
            .node("noise", Noise::value().period(2))
            .output(PbrOutput::new().base_color("noise"))
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("test:user")
            .node(
                "inner",
                Subgraph::new("test:flat").output(SurfaceOutput::Emissive),
            )
            .output(PbrOutput::new().base_color("inner"))
            .into_graph(),
    );
    let error = library.build("test:user").expect_err("no emissive");
    assert_eq!(error.path, "graphs[test:user].nodes[inner].output");
    assert!(error.reason.contains("emissive"), "{error}");
}

#[test]
fn a_parameter_carries_its_type_into_the_graph_and_an_integer_widens() {
    let material = MaterialGraph::builder("test:params")
        .param(Param::int("count", 3))
        .param(Param::bool("wet", true))
        .param(Param::color("tint", [0.2, 0.3, 0.4]))
        .node("noise", Noise::value().period(2))
        .node(
            "tinted",
            Blend::new(BlendMode::Multiply, "noise", Input::param("tint")),
        )
        .node(
            "switched",
            ashlar_material::nodes::Switch::new(Input::param("wet"), "tinted", "noise"),
        )
        .output(
            PbrOutput::new()
                .base_color("switched")
                .roughness(Input::param("count")),
        )
        .build()
        .expect("valid");
    assert_eq!(
        material.port("tinted").map(|p| p.value_type),
        Some(ValueType::Color),
    );
    assert_eq!(
        material.param("count").map(|p| p.value.value()),
        Some(Value::Float(3.0)),
    );
    assert_eq!(
        material.type_of(&Input::param("wet")),
        Some(ValueType::Float),
    );
    assert_eq!(ParamValue::Bool(false).value(), Value::Float(0.0));
}

#[test]
fn a_free_field_is_only_an_error_where_it_reaches_the_output() {
    // A quarter turn keeps the period; anything else does not.
    let turned = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value().periods(4, 8))
        .node("rot", Transform::new("noise").rotate(90.0))
        .output(PbrOutput::new().base_color("rot"))
        .build()
        .expect("a quarter turn tiles");
    assert_eq!(
        turned.port("rot").map(|p| p.period),
        Some(Period::Tiled { u: 8, v: 4 })
    );

    let free = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value().period(4))
        .node("rot", Transform::new("noise").rotate(33.0))
        .node("kept", Levels::new("rot"))
        .output(PbrOutput::new().base_color("kept"))
        .into_graph();
    let error = rejects(free);
    // The path names the node that broke it, not the one that passed it on.
    assert_eq!(error.path, "nodes[rot]");
    assert!(error.reason.contains("quarter turn"), "{error}");
    assert!(error.reason.contains("output must tile"), "{error}");

    // A free field nothing reads is nobody's problem.
    let unread = MaterialGraph::builder("test:graph")
        .node("noise", Noise::value().period(4))
        .node("rot", Transform::new("noise").rotate(33.0))
        .output(PbrOutput::new().base_color("noise"))
        .build()
        .expect("an unread free field is not an error");
    assert_eq!(unread.port("rot").map(|p| p.period), Some(Period::Free));
}

#[test]
fn a_material_answers_for_every_input_shape_a_lowering_will_ask_about() {
    let material: Material = MaterialGraph::builder("test:graph")
        .param(Param::float("wear", 0.5))
        .node("noise", Noise::value().period(4))
        .node("warp", Warp::new("noise", Uv::new()).amount(0.1))
        .node("folded", Mirror::new("warp"))
        .output(PbrOutput::new().base_color("folded"))
        .build()
        .expect("valid");
    assert_eq!(material.type_of(&Input::float(1.0)), Some(ValueType::Float));
    assert_eq!(
        material.type_of(&Input::color([1.0; 3])),
        Some(ValueType::Color)
    );
    assert_eq!(
        material.type_of(&Input::param("wear")),
        Some(ValueType::Float)
    );
    assert_eq!(material.type_of(&Input::param("absent")), None);
    assert_eq!(
        material.type_of(&Input::node("noise")),
        Some(ValueType::Float)
    );
    assert_eq!(material.type_of(&Input::node("absent")), None);
    // Dependencies come before the nodes that read them, whatever order they
    // were authored in.
    let order = material.order();
    let at = |id: &str| order.iter().position(|node| node == id).expect("in order");
    assert!(at("noise") < at("warp"));
    assert!(at("warp") < at("folded"));
}

/// A library holding one compound that declares two inputs and exports a mask.
///
/// `height` is a float and `tint` a colour, so both the conversion that is
/// allowed and the one that is not have a port to arrive at, and the graph
/// exports `mask` beside its channels for the cases about auxiliary outputs.
fn compounds() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:compound")
            .node("height", GraphInput::float("height", 0.5))
            .node("tint", GraphInput::color("tint", [0.5; 3]))
            .node("worn", Blend::new(BlendMode::Multiply, "tint", "height"))
            .output(
                PbrOutput::new()
                    .base_color("worn")
                    .roughness("height")
                    .extra("mask", "height"),
            )
            .into_graph(),
    );
    library
}

/// A graph instancing that compound through the node the caller hands in.
fn instancing(node: Subgraph) -> Result<Material, GraphError> {
    MaterialGraph::builder("test:user")
        .node("noise", Noise::value().period(4))
        .node("inner", node)
        .output(PbrOutput::new().base_color("inner"))
        .into_graph()
        .build_in(&compounds())
}

#[test]
fn a_subgraph_binds_only_inputs_the_graph_declares_and_only_types_they_admit() {
    instancing(
        Subgraph::new("test:compound")
            .input("height", "noise")
            .input("tint", Value::Color([0.2, 0.3, 0.4])),
    )
    .expect("two names the compound declares, at the types it declares");

    // A misspelled name would otherwise wire a field into nothing and leave
    // the input on its default, so the node would look wired and the picture
    // would be the one it had before. The message says what there is instead.
    let error = instancing(Subgraph::new("test:compound").input("heigth", "noise"))
        .expect_err("no such input");
    assert_eq!(error.path, "nodes[inner].inputs[heigth]");
    assert!(
        error
            .reason
            .contains(r#"has no input "heigth"; it declares "height" (Float), "tint" (Color)"#),
        "{error}"
    );

    // A colour reaching a float input is the conversion the graph allows
    // everywhere else; a `Vec2` is the one that converts to nothing at all.
    instancing(Subgraph::new("test:compound").input("height", Value::Color([0.2, 0.3, 0.4])))
        .expect("a colour reaches a float input as its luminance");
    let error = instancing(Subgraph::new("test:compound").input("height", Input::vec2([0.5, 0.5])))
        .expect_err("a vec2 converts to nothing");
    assert_eq!(error.path, "nodes[inner].inputs[height]");
    assert!(
        error
            .reason
            .contains(r#"takes a Float for input "height", not a Vec2"#),
        "{error}"
    );

    // A node written inline in an input is hoisted under `<id>.<name>`, so a
    // name holding a dot generates an id that reads as another node's port.
    // That is refused as the name it is, rather than left to surface as a
    // duplicate id in a graph nobody wrote.
    let error = instancing(Subgraph::new("test:compound").input("height.mask", "noise"))
        .expect_err("a dot in an input name");
    assert_eq!(error.path, "nodes[inner].inputs[height.mask]");
    assert!(error.reason.contains("may not hold a '.'"), "{error}");

    // And a graph that declares nothing says so rather than listing an empty
    // set, because the author's next question is whether they have the right
    // key at all.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:plain")
            .node("noise", Noise::value().period(4))
            .output(PbrOutput::new().base_color("noise"))
            .into_graph(),
    );
    let error = MaterialGraph::builder("test:user")
        .node("noise", Noise::value().period(4))
        .node(
            "inner",
            Subgraph::new("test:plain").input("height", "noise"),
        )
        .output(PbrOutput::new().base_color("inner"))
        .into_graph()
        .build_in(&library)
        .expect_err("no such input");
    assert!(error.reason.contains("it declares none"), "{error}");
}

#[test]
fn two_inputs_of_one_graph_may_not_share_a_name() {
    // The name is what a node binds to, so two inputs answering to one name
    // would make the binding depend on which node the map happened to visit
    // first. Reported at the second of them, with the node that got there
    // first, because that is the one an author has to go and look at.
    let error = MaterialGraph::builder("test:twice")
        .node("a", GraphInput::float("wet", 0.25))
        .node("b", GraphInput::float("wet", 0.75))
        .node("mix", Blend::new(BlendMode::Add, "a", "b"))
        .output(PbrOutput::new().base_color("mix"))
        .build()
        .expect_err("two inputs of one name");
    assert_eq!(error.path, "nodes[b].name");
    assert!(
        error
            .reason
            .contains(r#"input "wet" is already declared by node "a""#),
        "{error}"
    );
    // Two names is two inputs, however alike the rest of the node is.
    MaterialGraph::builder("test:twice")
        .node("a", GraphInput::float("wet", 0.25))
        .node("b", GraphInput::float("dry", 0.75))
        .node("mix", Blend::new(BlendMode::Add, "a", "b"))
        .output(PbrOutput::new().base_color("mix"))
        .build()
        .expect("two inputs, two names");
}

#[test]
fn an_extra_output_takes_a_name_of_its_own_and_is_read_by_that_name() {
    // An extra is an output port like the six channels: it is validated the
    // same way, it answers under its own name, and it shares their namespace.
    let material = MaterialGraph::builder("test:extra")
        .node("noise", Noise::value().period(4))
        .output(PbrOutput::new().roughness("noise").extra("mask", "noise"))
        .build()
        .expect("valid");
    assert_eq!(
        material.output_port("mask").map(|port| port.value_type),
        Some(ValueType::Float)
    );
    assert_eq!(material.period(), Period::square(4));

    let taken = MaterialGraph::builder("test:extra")
        .node("noise", Noise::value().period(4))
        .output(
            PbrOutput::new()
                .roughness("noise")
                .extra("roughness", "noise"),
        )
        .build()
        .expect_err("a channel is not an extra");
    assert_eq!(taken.path, "output.extra[roughness]");
    assert!(taken.reason.contains("is a PBR channel"), "{taken}");

    let blank = MaterialGraph::builder("test:extra")
        .node("noise", Noise::value().period(4))
        .output(PbrOutput::new().roughness("noise").extra("  ", "noise"))
        .build()
        .expect_err("a blank name");
    assert_eq!(blank.path, "output.extra[  ]");
    assert!(blank.reason.contains("must not be blank"), "{blank}");
}

#[test]
fn reading_an_extra_a_subgraph_does_not_export_is_refused_with_what_it_does() {
    // The six channels are a closed set an author can read off the type. An
    // export is a name somebody chose inside a graph the caller may never have
    // opened, so the refusal says what that graph does export.
    let error =
        instancing(Subgraph::new("test:compound").output(SurfaceOutput::Extra("wet".to_owned())))
            .expect_err("no such export");
    assert_eq!(error.path, "nodes[inner].output");
    assert!(
        error
            .reason
            .contains(r#"binds no extra output "wet"; it exports "mask""#),
        "{error}"
    );
    // The name it does export resolves, and carries what the source carries.
    let material =
        instancing(Subgraph::new("test:compound").output(SurfaceOutput::Extra("mask".to_owned())))
            .expect("the name the compound exports");
    assert_eq!(
        material.port("inner").map(|port| port.value_type),
        Some(ValueType::Float)
    );
}

#[test]
fn the_lattice_of_a_bound_field_is_counted_by_the_graph_that_bound_it() {
    // A compound is inlined at lowering, so what the field laid inside it is
    // laid here. A graph that only counted its own nodes would resolve a
    // fraction of what it wrote, and the bake that refuses this resolution is
    // in `tests/bake.rs`.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:pass")
            .node("field", GraphInput::float("field", 0.5))
            .output(PbrOutput::new().roughness("field"))
            .into_graph(),
    );
    let material = MaterialGraph::builder("test:user")
        .node("fine", Noise::value().period(512))
        .node(
            "inner",
            Subgraph::new("test:pass")
                .input("field", "fine")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph()
        .build_in(&library)
        .expect("valid");
    assert_eq!(material.finest_lattice(), [512, 512]);
}

#[test]
fn two_nodes_binding_different_fields_never_share_one_instance() {
    // An instance is what a graph was inferred under, so two nodes that bind
    // different fields must get two of them. An input name is an author's
    // string and may hold whatever an author types, punctuation included, so
    // the signature an instance is keyed by has to stay unambiguous under a
    // name that reads like a signature itself: `odd` here is spelled exactly
    // as the entries for `x` and `y` are written down. A key that lost that
    // distinction would hand the second node the first node's inference, and
    // the second material would report a period nothing in it has.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:collide")
            .node("x", GraphInput::float("x", 0.5))
            .node("y", GraphInput::float("y", 0.5))
            .node("odd", GraphInput::float("x=Float 8x8 8x8;y", 0.5))
            .node("mix", Blend::new(BlendMode::Add, "y", "odd"))
            .output(PbrOutput::new().base_color("mix").roughness("x"))
            .into_graph(),
    );
    let material = MaterialGraph::builder("test:user")
        .node("noise", Noise::value().period(8))
        .node(
            "wired",
            Subgraph::new("test:collide")
                .input("x", "noise")
                .input("y", Value::Float(0.5))
                .output(SurfaceOutput::Roughness),
        )
        .node(
            "odd",
            Subgraph::new("test:collide")
                .input("x=Float 8x8 8x8;y", Value::Float(0.5))
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().base_color("wired").roughness("odd"))
        .into_graph()
        .build_in(&library)
        .expect("valid");
    // The node that bound the noise reads what the noise laid; the node that
    // bound one float to one oddly named input reads its own defaults.
    assert_eq!(
        material.port("wired").map(|port| port.period),
        Some(Period::square(8))
    );
    assert_eq!(
        material.port("odd").map(|port| port.period),
        Some(Period::UNIT)
    );
    assert_eq!(material.period(), Period::square(8));
}

#[test]
fn a_layer_writes_one_subgraph_node_per_output_under_the_ports_own_id() {
    let compound = MaterialGraph::builder("test:compound")
        .param(Param::float("amount", 0.5))
        .node("height", GraphInput::float("height", 0.5))
        .node("worn", Invert::new("height"))
        .output(
            PbrOutput::new()
                .base_color("worn")
                .roughness("worn")
                .height("worn")
                .extra("mask", "worn"),
        )
        .into_graph();
    let outputs = [
        SurfaceOutput::BaseColor,
        SurfaceOutput::Height,
        SurfaceOutput::Extra("mask".to_owned()),
    ];
    let user = MaterialGraph::builder("test:user")
        .node("grain", Noise::value().period(8))
        .layer(
            "wear",
            Subgraph::new("test:compound")
                .input("height", "grain")
                .param("amount", ParamValue::Float(0.25)),
            &outputs,
        )
        .output(
            PbrOutput::new()
                .base_color("wear.base_color")
                .height("wear.height")
                .roughness("wear.mask")
                .normal_strength(0.01),
        )
        .into_graph();

    // One node per output, named for the port it reads, and nothing else
    // added: the ids are the whole of the interface, since the output binds
    // them by hand.
    let layered: Vec<&String> = user
        .nodes
        .keys()
        .filter(|id| id.starts_with("wear."))
        .collect();
    assert_eq!(layered, ["wear.base_color", "wear.height", "wear.mask"]);
    for output in &outputs {
        let id = format!("wear.{}", output.port());
        let Some(Node::Subgraph(node)) = user.nodes.get(&id) else {
            panic!("{id} is not a subgraph node");
        };
        assert_eq!(&node.output, output, "{id} reads the wrong output");
        // Everything but the output is the one node the caller wrote, copied:
        // an instance is inferred per binding signature, so two copies that
        // disagreed about what they bound would be two instances.
        assert_eq!(node.graph, "test:compound");
        assert_eq!(node.inputs.len(), 1);
        assert!(node.inputs.contains_key("height"));
        assert_eq!(node.params[&"amount".to_owned()], ParamValue::Float(0.25));
    }

    let mut library = MaterialGraphLibrary::default();
    library.insert(compound);
    let built = user.build_in(&library).expect("the ids resolve");
    // And they are ids like any others: the output read them, and the period
    // of the bound field came back out through every one of them.
    for output in &outputs {
        let id = format!("wear.{}", output.port());
        assert_eq!(
            built.port(&id).map(|port| port.period),
            Some(Period::square(8)),
            "{id} did not resolve"
        );
    }
}
