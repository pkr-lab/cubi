// Regeln aus pkr-lab/.github/.github/workflows/commitlint.yml@v1.2.0
module.exports = {
  rules: {
    "type-enum": [2, "always", ["feat", "fix", "docs", "chore", "refactor", "test", "ci", "build", "perf", "style", "revert"]],
    "type-empty": [2, "never"],
    "type-case": [2, "always", "lower-case"],
    "scope-case": [2, "always", "lower-case"],
    "subject-empty": [2, "never"],
    "subject-full-stop": [2, "never", "."],
    "header-max-length": [2, "always", 120],
  },
};
