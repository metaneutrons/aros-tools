## Problem and contract

Describe the user-visible problem, the affected contract, and why this package
or boundary owns the change.

## Solution

Explain the implementation and its failure behavior. Call out compatibility,
migration, security, publication, or destructive-operation consequences.

## Verification

List exact commands, fixtures, host architectures, and physical boards tested.
Use “not applicable” with a reason where a matrix lane cannot apply.

## Checklist

- [ ] The pull-request title follows Conventional Commits and represents the squash commit.
- [ ] Tests cover the success path, failure path, stable diagnostic, and any mutation boundary.
- [ ] Existing destinations and user state remain safe on error or interruption.
- [ ] Pristine upstream AROS and AROS-NX behavior remain explicitly separated.
- [ ] User and developer documentation matches the implemented behavior.
- [ ] No credential, private URL, personal data, assistant attribution, or `Co-Authored-By` trailer is included.
- [ ] Formatting, architecture, Clippy, Rustdoc, audit, deny, tests, and relevant documentation/release gates pass.
