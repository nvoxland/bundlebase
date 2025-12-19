"""Awaitable operation chaining for Bundle.

Allows multiple operations to be queued and executed with a single await:

    c = await (c.attach("data.parquet")
              .remove_column("col")
              .rename_column("old", "new"))

Also supports chaining from create() and open():

    c = await (create(path)
              .attach("data.parquet")
              .remove_column("unwanted"))
"""

from typing import Any, List, Tuple, Callable, Dict, Optional

# Store original methods to avoid recursion
_ORIGINAL_METHODS: Dict[str, Callable] = {}


def register_original_method(name: str, method: Callable) -> None:
    """Register the original (non-wrapped) method."""
    _ORIGINAL_METHODS[name] = method


class OperationChain:
    """Chains multiple async operations on a Bundle with a single await.

    This class queues up method calls and executes them all when awaited,
    allowing for fluent chaining with a single await at the end.

    Example:
        c = await (c.attach("data.parquet")
                  .remove_column("unwanted")
                  .rename_column("old_name", "new_name"))
    """

    def __init__(self, bundle: Any):
        """Initialize the chain with a bundle.

        Args:
            bundle: PyBundleBuilder or PyBundle instance
        """
        self._bundle = bundle
        self._operations: List[Tuple[str, tuple, dict]] = []
        self._executed = False

    def __getattr__(self, name: str) -> Any:
        """Intercept method calls to queue operations or handle final conversions.

        Args:
            name: Method name

        Returns:
            A callable that either queues the operation or executes the chain

        Raises:
            AttributeError: If the method name is not a valid operation
        """
        if name.startswith("_"):
            # Don't intercept private attributes
            raise AttributeError(f"'{type(self).__name__}' object has no attribute '{name}'")

        # Conversion and streaming methods should execute the chain, not queue operations
        conversion_methods = {"to_pandas", "to_polars", "to_numpy", "to_dict", "as_pyarrow_stream"}
        if name in conversion_methods:
            async def conversion_wrapper(*args, **kwargs):
                # Execute the chain first
                bundle = await self._execute()
                # Then call the conversion method on the result
                method = getattr(bundle, name)
                return await method(*args, **kwargs)

            return conversion_wrapper

        # Validate that the method is a known operation before queuing
        # This gives early feedback instead of failing at await time
        original_method = _ORIGINAL_METHODS.get(name)
        if original_method is None:
            # Provide helpful error message with available methods
            available_methods = sorted([
                m for m in _ORIGINAL_METHODS.keys() if not m.startswith("_")
            ])
            raise AttributeError(
                f"'{type(self).__name__}' has no method '{name}'. "
                f"Available methods: {', '.join(available_methods)}"
            )

        # Validate that the method is a known operation before queuing
        # This gives early feedback instead of failing at await time
        original_method = _ORIGINAL_METHODS.get(name)
        if original_method is None:
            # Provide helpful error message with available methods
            available_methods = sorted([
                m for m in _ORIGINAL_METHODS.keys() if not m.startswith("_")
            ])
            raise AttributeError(
                f"'{type(self).__name__}' has no method '{name}'. "
                f"Available methods: {', '.join(available_methods)}"
            )

        # Other methods are queued as operations
        def method_wrapper(*args, **kwargs):
            # Queue the operation
            self._operations.append((name, args, kwargs))
            return self

        return method_wrapper

    def __await__(self):
        """Make this chain awaitable.

        Executes all queued operations in order and returns the final bundle.
        """
        return self._execute().__await__()

    async def _execute(self) -> Any:
        """Execute all queued operations and return the final bundle."""
        self._executed = True
        bundle = self._bundle

        for method_name, args, kwargs in self._operations:
            # Use the original method to avoid recursion
            original_method = _ORIGINAL_METHODS.get(method_name)
            if original_method:
                result = original_method(bundle, *args, **kwargs)
            else:
                # Fallback to getattr (for non-wrapped methods)
                method = getattr(bundle, method_name)
                result = method(*args, **kwargs)

            bundle = await result

        return bundle

    def __del__(self):
        """Detect if operations were never executed.

        Logs a warning if the chain had queued operations that were
        never executed (i.e., the chain was never awaited).
        """
        if not self._executed and self._operations:
            import warnings
            # Format a helpful warning message
            op_names = ", ".join(op[0] for op in self._operations)
            warnings.warn(
                f"OperationChain with {len(self._operations)} operation(s) "
                f"was never awaited: {op_names}. "
                f"Did you forget to add 'await' before the chain?",
                RuntimeWarning,
                stacklevel=2
            )


class CreateChain:
    """
    Awaitable chain for fluent creation and chaining in one go.

    Handles the special case of creating a bundle first, then chaining operations.
    Unlike OperationChain, this starts without a bundle and creates one first.

    Example:
        c = await (create(path)
                  .attach("data.parquet")
                  .remove_column("unwanted"))
    """

    def __init__(self, create_func: Callable, *create_args):
        """
        Initialize the creation chain.

        Args:
            create_func: The async create or open function from Rust bindings
            *create_args: Arguments to pass to the create function
        """
        self._bundle: Optional[Any] = None
        self._operations: List[Tuple[str, tuple, dict]] = []
        self._create_func = create_func
        self._create_args = create_args
        self._executed = False

    def __getattr__(self, name: str) -> Any:
        """Intercept method calls to queue operations or handle final conversions.

        Args:
            name: Method name

        Returns:
            A callable that either queues the operation or executes the chain
        """
        if name.startswith("_"):
            # Don't intercept private attributes
            raise AttributeError(f"'{type(self).__name__}' object has no attribute '{name}'")

        # Conversion and streaming methods should execute the chain, not queue operations
        conversion_methods = {"to_pandas", "to_polars", "to_numpy", "to_dict", "as_pyarrow_stream"}
        if name in conversion_methods:
            async def conversion_wrapper(*args, **kwargs):
                # Execute the chain first
                bundle = await self._execute()
                # Then call the conversion method on the result
                method = getattr(bundle, name)
                return await method(*args, **kwargs)

            return conversion_wrapper

        # Validate that the method is a known operation before queuing
        # This gives early feedback instead of failing at await time
        original_method = _ORIGINAL_METHODS.get(name)
        if original_method is None:
            # Provide helpful error message with available methods
            available_methods = sorted([
                m for m in _ORIGINAL_METHODS.keys() if not m.startswith("_")
            ])
            raise AttributeError(
                f"'{type(self).__name__}' has no method '{name}'. "
                f"Available methods: {', '.join(available_methods)}"
            )

        # Other methods are queued as operations
        def method_wrapper(*args, **kwargs):
            # Queue the operation
            self._operations.append((name, args, kwargs))
            return self

        return method_wrapper

    def __await__(self):
        """Make this chain awaitable.

        Executes all queued operations and returns the final bundle.
        """
        return self._execute().__await__()

    async def _execute(self) -> Any:
        """Execute the creation first, then all queued operations."""
        self._executed = True
        # First, create the bundle
        original_create = _ORIGINAL_METHODS.get("create") or _ORIGINAL_METHODS.get("open")
        if original_create is None:
            raise RuntimeError("No create/open function registered")

        # Call the create function with its arguments
        bundle = await self._create_func(*self._create_args)

        # Then execute all queued operations
        for method_name, args, kwargs in self._operations:
            # Use the original method to avoid recursion
            original_method = _ORIGINAL_METHODS.get(method_name)
            if original_method:
                result = original_method(bundle, *args, **kwargs)
            else:
                # Fallback to getattr (for non-wrapped methods)
                method = getattr(bundle, method_name)
                result = method(*args, **kwargs)

            bundle = await result

        return bundle

    def __del__(self):
        """Detect if create/open was never executed.

        Logs a warning if the creation chain was never awaited.
        """
        if not self._executed:
            import warnings
            func_name = "create" if "create" in str(self._create_func) else "open"

            if self._operations:
                op_names = ", ".join(op[0] for op in self._operations)
                message = (
                    f"CreateChain starting with {func_name}() followed by "
                    f"{len(self._operations)} operation(s) was never awaited: {op_names}. "
                    f"Did you forget to add 'await' before {func_name}()?"
                )
            else:
                message = (
                    f"{func_name}() was never awaited. "
                    f"Did you forget to add 'await' before {func_name}()?"
                )

            warnings.warn(message, RuntimeWarning, stacklevel=2)


class ExtendChain:
    """
    Awaitable chain for extending an existing bundle to a new directory.

    Handles the special case of extending an existing bundle, then chaining operations.
    Unlike OperationChain, extend() is synchronous and returns immediately, allowing
    chaining to begin without awaiting first.

    Example:
        c = await (existing_c.extend("/new/path")
                          .attach("new_data.parquet")
                          .remove_column("unwanted"))
    """

    def __init__(self, original_extend_method: Callable, existing_bundle: Any, data_dir: str):
        """
        Initialize the extend chain.

        Args:
            original_extend_method: The original (unwrapped) extend method from Rust
            existing_bundle: The PyBundle to extend
            data_dir: The directory path for the new extended bundle
        """
        self._bundle: Any = original_extend_method(existing_bundle, data_dir)
        self._operations: List[Tuple[str, tuple, dict]] = []
        self._executed = False

    def __getattr__(self, name: str) -> Any:
        """Intercept method calls to queue operations or handle final conversions.

        Args:
            name: Method name

        Returns:
            A callable that either queues the operation or executes the chain
        """
        if name.startswith("_"):
            # Don't intercept private attributes
            raise AttributeError(f"'{type(self).__name__}' object has no attribute '{name}'")

        # Conversion and streaming methods should execute the chain, not queue operations
        conversion_methods = {"to_pandas", "to_polars", "to_numpy", "to_dict", "as_pyarrow_stream"}
        if name in conversion_methods:
            async def conversion_wrapper(*args, **kwargs):
                # Execute the chain first
                bundle = await self._execute()
                # Then call the conversion method on the result
                method = getattr(bundle, name)
                return await method(*args, **kwargs)

            return conversion_wrapper

        # Other methods are queued as operations
        def method_wrapper(*args, **kwargs):
            # Queue the operation
            self._operations.append((name, args, kwargs))
            return self

        return method_wrapper

    def __await__(self):
        """Make this chain awaitable.

        Executes all queued operations and returns the final bundle.
        """
        return self._execute().__await__()

    async def _execute(self) -> Any:
        """Execute all queued operations and return the final bundle."""
        self._executed = True
        bundle = self._bundle

        for method_name, args, kwargs in self._operations:
            # Use the original method to avoid recursion
            original_method = _ORIGINAL_METHODS.get(method_name)
            if original_method:
                result = original_method(bundle, *args, **kwargs)
            else:
                # Fallback to getattr (for non-wrapped methods)
                method = getattr(bundle, method_name)
                result = method(*args, **kwargs)

            bundle = await result

        return bundle

    def __del__(self):
        """Detect if extend chain was never executed.

        Logs a warning if the extend chain was never awaited.
        """
        if not self._executed and self._operations:
            import warnings
            op_names = ", ".join(op[0] for op in self._operations)
            warnings.warn(
                f"ExtendChain with {len(self._operations)} operation(s) "
                f"was never awaited: {op_names}. "
                f"Did you forget to add 'await' after extend()?",
                RuntimeWarning,
                stacklevel=2
            )
