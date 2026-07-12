"""Stub (.pyi) accuracy tests — bb-cko.2.

The type stub must describe the actual runtime surface: every method the
stub declares must exist at runtime, async-ness must match reality, and
the package must ship a ``py.typed`` marker so PEP 561 checkers actually
use the stubs.
"""

import ast
from pathlib import Path

import bundlebase

STUB_PATH = Path(bundlebase.__file__).with_name("__init__.pyi")

# Dunders implemented by the chain wrappers themselves, not proxied.
CHAIN_OWN_METHODS = {"__init__", "__getattr__", "__await__", "__del__"}


def _stub_classes():
    tree = ast.parse(STUB_PATH.read_text())
    return {node.name: node for node in tree.body if isinstance(node, ast.ClassDef)}


def _stub_methods(cls_node):
    """Map of method name -> {'async': bool, 'property': bool, 'returns': str|None}."""
    methods = {}
    for node in cls_node.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            decorators = {
                d.id for d in node.decorator_list if isinstance(d, ast.Name)
            }
            methods[node.name] = {
                "async": isinstance(node, ast.AsyncFunctionDef),
                "property": "property" in decorators,
                "returns": ast.unparse(node.returns) if node.returns else None,
            }
    return methods


def test_stub_file_and_classes_present():
    assert STUB_PATH.exists()
    classes = _stub_classes()
    assert "PyBundle" in classes
    assert "PyBundleBuilder" in classes


def test_pybundle_stub_methods_exist_at_runtime():
    stub = _stub_methods(_stub_classes()["PyBundle"])
    missing = [name for name in stub if not hasattr(bundlebase.PyBundle, name)]
    assert missing == [], f"Stub declares methods PyBundle does not have: {missing}"


def test_pybundlebuilder_stub_methods_exist_at_runtime():
    stub = _stub_methods(_stub_classes()["PyBundleBuilder"])
    missing = [name for name in stub if not hasattr(bundlebase.PyBundleBuilder, name)]
    assert missing == [], (
        f"Stub declares methods PyBundleBuilder does not have: {missing}"
    )


def test_operationchain_stub_methods_are_proxyable():
    """OperationChain proxies to PyBundleBuilder — every stubbed method must
    resolve there (or be one of the chain's own dunders)."""
    stub = _stub_methods(_stub_classes()["OperationChain"])
    missing = [
        name
        for name in stub
        if name not in CHAIN_OWN_METHODS
        and not hasattr(bundlebase.PyBundleBuilder, name)
    ]
    assert missing == [], (
        f"OperationChain stub declares methods the builder does not have: {missing}"
    )


def test_pyrecordbatchstream_stub_methods_exist_at_runtime():
    from bundlebase import _bundlebase

    stub = _stub_methods(_stub_classes()["PyRecordBatchStream"])
    runtime = _bundlebase.PyRecordBatchStream
    missing = [name for name in stub if not hasattr(runtime, name)]
    assert missing == [], (
        f"Stub declares methods PyRecordBatchStream does not have: {missing}"
    )


def test_query_declared_in_stub():
    classes = _stub_classes()
    for cls in ("PyBundle", "PyBundleBuilder"):
        assert "query" in _stub_methods(classes[cls]), f"{cls} stub omits query()"


def test_num_rows_is_awaitable_method_not_sync_property():
    classes = _stub_classes()
    for cls in ("PyBundle", "PyBundleBuilder"):
        info = _stub_methods(classes[cls])["num_rows"]
        assert not info["property"], f"{cls}.num_rows is awaitable, not a property"
        assert info["async"], f"{cls}.num_rows must be declared async"


def test_commit_returns_none():
    info = _stub_methods(_stub_classes()["PyBundleBuilder"])["commit"]
    assert info["returns"] == "None", (
        f"commit() resolves to None at runtime, stub says {info['returns']}"
    )


def test_py_typed_marker_shipped():
    marker = Path(bundlebase.__file__).with_name("py.typed")
    assert marker.exists(), "py.typed missing — PEP 561 tools ignore the stubs"


def test_runtime_public_surface_is_fully_stubbed():
    """Every public runtime method must be declared in the stub — omissions
    make type checkers reject valid user code (bb-bos.2 fallout: mkdocstrings
    renders from the stub, so undeclared methods vanish from the API docs)."""
    for cls_name in ("PyBundle", "PyBundleBuilder"):
        stub = set(_stub_methods(_stub_classes()[cls_name]))
        runtime = {
            n for n in dir(getattr(bundlebase, cls_name)) if not n.startswith("_")
        }
        missing = sorted(runtime - stub)
        assert missing == [], f"{cls_name} stub omits runtime methods: {missing}"
