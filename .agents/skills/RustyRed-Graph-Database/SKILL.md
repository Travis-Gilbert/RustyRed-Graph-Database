```markdown
# RustyRed-Graph-Database Development Patterns

> Auto-generated skill from repository analysis

## Overview
This skill provides guidance on contributing to the RustyRed-Graph-Database project, a Rust-based graph database implementation. It documents the project's coding conventions, commit patterns, and testing approaches to ensure consistency and maintainability. This guide is ideal for developers looking to contribute code, write tests, or maintain the repository.

## Coding Conventions

### File Naming
- Use **camelCase** for file names.
  - Example: `graphEngine.rs`, `nodeManager.rs`

### Import Style
- Use **relative imports** for referencing modules within the project.
  - Example:
    ```rust
    mod nodeManager;
    use crate::nodeManager::Node;
    ```

### Export Style
- Use **named exports** to expose specific structs, enums, or functions.
  - Example:
    ```rust
    pub struct Graph { /* ... */ }
    pub fn create_node() { /* ... */ }
    ```

### Commit Messages
- Follow **conventional commit** style.
- Use the `feat` prefix for new features.
- Keep commit messages concise (average ~38 characters).
  - Example:  
    ```
    feat: add edge deletion support
    ```

## Workflows

### Feature Development
**Trigger:** When adding a new feature to the codebase  
**Command:** `/feature-development`

1. Create a new branch for your feature.
2. Implement the feature following the coding conventions.
3. Write or update tests in files matching `*.test.*`.
4. Commit your changes using the `feat` prefix and a concise message.
5. Submit a pull request for review.

### Testing Code
**Trigger:** When verifying code correctness  
**Command:** `/run-tests`

1. Identify or create test files using the `*.test.*` pattern.
2. Run tests using the project's preferred testing command (framework unknown; typically `cargo test` for Rust).
3. Review test results and fix any failing cases.

## Testing Patterns

- Test files follow the `*.test.*` naming convention (e.g., `graphEngine.test.rs`).
- The specific testing framework is not detected, but Rust's built-in test framework is likely.
- Example test module:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn test_create_node() {
          let node = create_node();
          assert!(node.is_valid());
      }
  }
  ```

## Commands
| Command              | Purpose                                  |
|----------------------|------------------------------------------|
| /feature-development | Start the feature development workflow   |
| /run-tests           | Run all tests in the repository          |
```