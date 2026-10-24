## What and why

## Checklist

- [ ] One change, explained above.
- [ ] Keeps the [safety model](../docs/safety-model.md):
  - [ ] any new request to OpenRGB, especially one that changes a light, is listed in the safety model in this pull request;
  - [ ] only lights the user has set are ever changed;
  - [ ] no OpenRGB profiles are loaded, saved or deleted;
  - [ ] any new file write is listed in the safety model;
  - [ ] no network access beyond the SDK connection;
  - [ ] no admin rights.
- [ ] Tests added or updated where the behaviour can be tested.
- [ ] UI changes:
  - [ ] checked in dark and light mode;
  - [ ] checked with the keyboard;
  - [ ] controls have screen-reader labels.
- [ ] README or docs updated if behaviour changed.

## Verification
