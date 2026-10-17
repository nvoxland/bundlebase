"""Thin smoke coverage of semantic dicts, chaining, paths, and sync bindings."""

import json

import pytest

import bundlebase
import bundlebase.sync as sync


@pytest.mark.asyncio
async def test_semantic_document_binding_roundtrip(tmp_path):
    data = tmp_path / "sales.csv"
    data.write_text("id,amount\n1,10\n2,20\n")
    builder = await (
        bundlebase.create(str(tmp_path / "bundle"))
        .attach(data.as_uri())
        .cast_column("amount", "Int64")
        .define_metric("revenue", "SUM(amount)")
        .set_column_label("amount", "Revenue")
        .set_metric_description("revenue", "Total sales")
        .set_ai_context("model", {"instructions": "Use revenue", "extra": [True, 1]})
    )
    document = await builder.semantic_model()
    assert isinstance(document, dict)
    assert document["ai_context"]["extra"] == [True, 1]
    document["description"] = "Sales model"
    preview = await builder.preview_semantic_model(document)
    assert preview
    assert all(isinstance(operation.op_type, str) for operation in preview)
    assert await builder.apply_semantic_model(document) is not None

    path = str(tmp_path / "model.yaml")
    await builder.export_semantic_model(path)
    await builder.set_description("Changed").import_semantic_model(path)
    assert (await builder.semantic_model())["description"] == "Sales model"

    # The new command dispatcher is available as a terminal chain operation.
    result = await builder.set_metric_expression("revenue", "SUM(amount)").execute(
        "DECLARE PRIMARY KEY (id)"
    )
    assert isinstance(result, bundlebase.QueryResult)
    await builder.commit("semantic bindings")
    readonly = await bundlebase.open(str(tmp_path / "bundle"))
    exported = str(tmp_path / "readonly.json")
    await readonly.export_semantic_model(exported)
    assert json.loads((tmp_path / "readonly.json").read_text()) == await readonly.semantic_model()


def test_sync_semantic_bindings_and_command_dispatch(tmp_path):
    data = tmp_path / "sales.csv"
    data.write_text("id,amount\n1,10\n")
    builder = (
        sync.create(str(tmp_path / "sync-bundle"))
        .attach(data.as_uri())
        .cast_column("amount", "Int64")
        .define_metric("revenue", "SUM(amount)")
        .set_column_label("amount", "Revenue")
        .set_metric_description("revenue", "Total")
        .set_metric_expression("revenue", "SUM(amount)")
        .set_ai_context("metric", {"examples": ["Total sales?"]}, name="revenue")
    )
    document = builder.semantic_model()
    document["description"] = "Sync sales"
    assert builder.preview_semantic_model(document)
    assert builder.apply_semantic_model(document) is builder
    path = str(tmp_path / "model.json")
    builder.export_semantic_model(path)
    assert builder.import_semantic_model(path) is builder
    builder.execute("ALTER COLUMN amount DROP LABEL")
    assert builder.semantic_model()["description"] == "Sync sales"


@pytest.mark.asyncio
async def test_semantic_python_argument_shapes(tmp_path):
    builder = await bundlebase.create(str(tmp_path / "arguments-bundle"))
    with pytest.raises(TypeError):
        await builder.apply_semantic_model("not a dict")
    with pytest.raises(ValueError, match="target"):
        await builder.set_ai_context("column", {}, name=None)
