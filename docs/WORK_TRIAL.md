# Work trial

Use this sequence for the first hands-on session.

## 1. Compile and verify

```bash
./scripts/check.sh
cargo install --path .
```

## 2. Install shell integration

```bash
acre setup
exec "$SHELL" -l
```

## 3. Use a safe repository

Choose a repository where all important work is committed and pushed. Check Acre's view first:

```bash
acre system doctor
acre system inspect
```

## 4. Prepare one idle slot

```bash
acre system warm --slots 1
```

## 5. Create trial work

```bash
acre new acre/trial
```

When Acre reports a cold environment, run the normal project setup yourself. Acre does not execute repository code automatically.

## 6. Exercise the daily flow

```bash
acre -
acre acre/trial
acre acre/trial -- git status --short
acre done
acre acre/trial
```

## 7. Observe the runtime

```bash
acre system inspect
acre system doctor
```

## Feedback worth recording

- Was `open / new / done` obvious without reading help?
- Did relative-directory preservation feel natural?
- Was the first warm reuse measurably useful?
- Did any safety refusal feel surprising or vague?
- Did Acre keep a workspace when you expected it to return, or return one you expected it to keep?
- Which output felt noisy, cheap, or too technical?
