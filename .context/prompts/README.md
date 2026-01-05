# Prompt Templates

This directory contains prompt templates for common bundlebase development tasks. These templates help AI assistants understand the project structure, constraints, and best practices when implementing features or fixing bugs.

## Purpose

Prompt templates:
- Provide consistent workflows for common tasks
- Encode bundlebase-specific knowledge and constraints
- Reduce errors by guiding AI through proper implementation steps
- Document best practices for development tasks

## Available Templates

### [new-feature.md](new-feature.md)
Use when: Adding a completely new feature to bundlebase

Examples:
- Adding a new data source adapter (JSON, CSV, etc.)
- Implementing a new query optimization
- Adding a new Python API method

### [add-operation.md](add-operation.md)
Use when: Adding a new operation to the operation pipeline

Examples:
- Adding `join()` operation
- Adding `sort()` operation
- Adding `aggregate()` operation

This is the most common template - operations are the building blocks of bundlebase transformations.

### [add-python-binding.md](add-python-binding.md)
Use when: Exposing existing Rust functionality to Python

Examples:
- Wrapping a new Rust method for Python access
- Adding Python-friendly API for existing functionality
- Creating async/sync bridge for new features

### [fix-bug.md](fix-bug.md)
Use when: Investigating and fixing a bug

Examples:
- Data corruption issues
- Memory leaks or performance problems
- Type errors or API mismatches
- Error handling failures

### [performance-review.md](performance-review.md)
Use when: Optimizing performance or reviewing for efficiency

Examples:
- Investigating slow queries
- Reducing memory usage
- Eliminating unnecessary data copies
- Ensuring streaming execution is used

## How to Use These Templates

### For AI Assistants

When starting a task that matches one of these templates:

1. **Read the template** - Use the Read tool to load the full template
2. **Follow the checklist** - Complete each step in order
3. **Reference documentation** - Read files mentioned in "Required Reading"
4. **Validate constraints** - Check all items in "Critical Constraints"
5. **Test thoroughly** - Follow testing steps specific to the task

### For Developers

When working with an AI assistant:

1. **Identify the task type** - Which template matches your work?
2. **Reference the template** - Point the AI to the specific template file
3. **Provide context** - Give specific details (feature name, bug description, etc.)
4. **Review the plan** - Check that AI followed the template workflow

Example:
```
"Add a sort() operation to bundlebase. Follow the add-operation.md template."
```

## Template Structure

Each template follows this structure:

1. **Task Description** - What this template is for
2. **Required Reading** - Documentation files to read before starting
3. **Critical Constraints** - Hard rules that must be followed
4. **Implementation Checklist** - Step-by-step workflow
5. **Testing Requirements** - How to verify the implementation
6. **Common Pitfalls** - Mistakes to avoid
7. **Examples** - Reference implementations

## When NOT to Use Templates

Templates are for **common, well-defined tasks**. Skip them for:

- Quick documentation fixes
- Trivial code changes (typos, formatting)
- Exploratory research or investigation
- One-off scripts or tools

## Customizing Templates

When a template doesn't quite fit:

1. **Start with the closest template** - Use it as a baseline
2. **Adapt the checklist** - Modify steps as needed
3. **Keep core constraints** - Always follow Critical Constraints section
4. **Document deviations** - Note why you diverged from template

## Related Documentation

- [ai-rules.md](../ai-rules.md) - Hard constraints for all AI code generation
- [anti-patterns.md](../anti-patterns.md) - What NOT to do
- [workflows.md](../workflows.md) - General development workflows
- [decisions/](../decisions/) - Architecture Decision Records explaining why things are the way they are

## Contributing Templates

To add a new template:

1. **Identify a repeated pattern** - Is this task done frequently?
2. **Extract the workflow** - What steps are always needed?
3. **Document constraints** - What rules must be followed?
4. **Add examples** - Reference existing code that follows the pattern
5. **Test with AI** - Does the template produce correct code?

Templates should be **specific enough to be useful** but **general enough to be reusable**.
