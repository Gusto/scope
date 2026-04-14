# Manual Test Plan: scope-intercept retry on fix (LDE-463)

## Setup

```bash
cargo build --bin scope-intercept

mkdir -p /tmp/test-intercept-retry/.scope
cd /tmp/test-intercept-retry

INTERCEPT=/path/to/target/debug/scope-intercept
```

Note: use `--` to separate scope-intercept flags from commands that have their own flags
(e.g. `scope-intercept --extra-config ... -- bash -c '...'`).

---

## Test 1: Fix succeeds, retry succeeds (happy path)

**Setup:**
```bash
cat > .scope/known-error.yaml << 'EOF'
apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: missing-ready-file
  description: Detects when ready.txt is missing and creates it
spec:
  pattern: "ready.txt: No such file"
  help: The ready.txt file is missing.
  fix:
    prompt:
      text: "Create the ready.txt file?"
    commands:
      - bash -c 'echo "ready" > ready.txt'
EOF

rm -f ready.txt
```

**Run** (requires a TTY for the fix prompt):
```bash
$INTERCEPT --extra-config /tmp/test-intercept-retry/.scope cat ready.txt
# When prompted "Create the ready.txt file?" → answer y
```

**Output:**
```
cat: ready.txt: No such file or directory
ERROR Command failed, checking for a known error
 WARN Known error 'missing-ready-file' found on line 0
 INFO     ==> The ready.txt file is missing.
 INFO found a fix!
? Create the ready.txt file? (y/N) y
 INFO All known errors detected, ignoring rest of output.
 INFO Fix succeeded
 INFO Fix succeeded, retrying command
ready
EXIT_CODE=0
```

**Result: PASS** — fix runs, command retried, prints "ready", exit 0.

---

## Test 2: Fix succeeds, retry still fails (one retry only)

**Setup:**
```bash
cat > check.sh << 'SCRIPT'
#!/bin/bash
set -e
cat ready.txt
cat other.txt
SCRIPT
chmod +x check.sh
rm -f ready.txt other.txt
```

**Run:**
```bash
$INTERCEPT --extra-config /tmp/test-intercept-retry/.scope bash check.sh
# When prompted → answer y
```

**Output:**
```
cat: ready.txt: No such file or directory
ERROR Command failed, checking for a known error
 WARN Known error 'missing-ready-file' found on line 0
 INFO     ==> The ready.txt file is missing.
 INFO found a fix!
? Create the ready.txt file? (y/N) y
 INFO All known errors detected, ignoring rest of output.
 INFO Fix succeeded
 INFO Fix succeeded, retrying command
ready
cat: other.txt: No such file or directory
EXIT_CODE=1
```

**Result: PASS** — fix runs, retry executes (prints "ready"), fails on `other.txt`, no second analysis, exit 1.

---

## Test 3: Known error found, no fix available

**Setup:**
```bash
cat > .scope/known-error.yaml << 'EOF'
apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: something-broke
  description: A known error with no automatic fix
spec:
  pattern: "something went wrong"
  help: "This is a known issue. Check the wiki for manual steps."
EOF
```

**Run:**
```bash
SCOPE_DISABLE_DEFAULT_CONFIG=true $INTERCEPT --extra-config /tmp/test-intercept-retry/.scope -- bash -c 'echo "something went wrong"; exit 1'
```

**Output:**
```
something went wrong
ERROR Command failed, checking for a known error
 WARN Known error 'something-broke' found on line 0
 INFO     ==> This is a known issue. Check the wiki for manual steps.
 INFO All known errors detected, ignoring rest of output.
 INFO No automatic fix available
EXIT_CODE=1
```

**Result: PASS** — known error and help text shown, "No automatic fix available", no prompt, no retry, exit 1.

---

## Test 4: User denies the fix

(Restore fix-enabled config from Test 1, `rm -f ready.txt` first)

**Run:**
```bash
$INTERCEPT --extra-config /tmp/test-intercept-retry/.scope cat ready.txt
# When prompted "Create the ready.txt file?" → answer n
```

**Output:**
```
cat: ready.txt: No such file or directory
ERROR Command failed, checking for a known error
 WARN Known error 'missing-ready-file' found on line 0
 INFO     ==> The ready.txt file is missing.
 INFO found a fix!
? Create the ready.txt file? (y/N) n
 INFO All known errors detected, ignoring rest of output.
 WARN User denied fix
EXIT_CODE=1
```

**Result: PASS** — user says No, "User denied fix", no retry, exit 1.

---

## Test 5: Command succeeds on first try

**Setup:** `echo "ready" > ready.txt`

**Run:**
```bash
SCOPE_DISABLE_DEFAULT_CONFIG=true $INTERCEPT --extra-config /tmp/test-intercept-retry/.scope cat ready.txt
```

**Output:**
```
ready
EXIT_CODE=0
```

**Result: PASS** — no error messages, no analysis, exit 0.

---

## Test 6: Command fails, no known errors match

**Run:**
```bash
SCOPE_DISABLE_DEFAULT_CONFIG=true $INTERCEPT --extra-config /tmp/test-intercept-retry/.scope -- bash -c 'echo "totally unexpected failure"; exit 42'
```

**Output:**
```
totally unexpected failure
ERROR Command failed, checking for a known error
 INFO No known errors found
EXIT_CODE=42
```

**Result: PASS** — "No known errors found", no retry, original exit code 42 preserved.

---

## Test 7: Shebang usage (primary intended use case)

scope-intercept is designed to be used as a script's shebang interpreter. The kernel passes the script path as an argument, so `scope-intercept bash` in the shebang causes it to run `bash <script>` and wrap the whole thing.

**Setup:**
```bash
mkdir -p /tmp/test-intercept-retry/.scope

cat > /tmp/test-intercept-retry/.scope/known-error.yaml << 'EOF'
apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: missing-ready-file
  description: Detects when ready.txt is missing and creates it
spec:
  pattern: "ready.txt: No such file"
  help: The ready.txt file is missing.
  fix:
    prompt:
      text: "Create the ready.txt file?"
    commands:
      - bash -c 'echo "ready" > ready.txt'
EOF

cat > /tmp/test-intercept-retry/setup.sh << 'EOF'
#!/path/to/target/debug/scope-intercept bash
set -e
echo "Running setup..."
cat ready.txt
echo "Setup complete!"
EOF
chmod +x /tmp/test-intercept-retry/setup.sh

cd /tmp/test-intercept-retry && rm -f ready.txt
```

**Run** (from within the directory so `.scope` is found automatically):
```bash
cd /tmp/test-intercept-retry
./setup.sh
# When prompted "Create the ready.txt file?" → answer y
```

**Output:**
```
Running setup...
cat: ready.txt: No such file or directory
ERROR Command failed, checking for a known error
 WARN Known error 'missing-ready-file' found on line 1
 INFO     ==> The ready.txt file is missing.
 INFO found a fix!
? Create the ready.txt file? (y/N) y
 INFO All known errors detected, ignoring rest of output.
 INFO Fix succeeded
 INFO Fix succeeded, retrying command
Running setup...
ready
Setup complete!
EXIT_CODE=0
```

**Result: PASS** — script runs, fails, fix applied, entire script retried from the top ("Running setup..." appears twice), exits 0.

Note: no `--extra-config` or `SCOPE_DISABLE_DEFAULT_CONFIG` needed — scope-intercept finds `.scope` in the working directory automatically.

---

## Cleanup

```bash
rm -rf /tmp/test-intercept-retry
```

---

## Summary

| # | Test | Result |
|---|------|--------|
| 1 | Fix succeeds, retry succeeds | PASS |
| 2 | Fix succeeds, retry still fails (one retry only) | PASS |
| 3 | Known error found, no fix available | PASS |
| 4 | User denies the fix | PASS |
| 5 | Command succeeds on first try | PASS |
| 6 | No known errors match, exit code preserved | PASS |
| 7 | Shebang usage (primary intended use case) | PASS |
