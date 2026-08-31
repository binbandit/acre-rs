use crate::model::SupportedShell;

pub fn generate_shell_integration(shell: SupportedShell) -> String {
    match shell {
        SupportedShell::Bash => posix_integration("bash"),
        SupportedShell::Zsh => posix_integration("zsh"),
        SupportedShell::Fish => fish_integration(),
        SupportedShell::Powershell => powershell_integration(),
    }
}

pub fn generate_completion(shell: SupportedShell) -> String {
    match shell {
        SupportedShell::Fish => r#"function __acre_complete
  set -l token (commandline -ct)
  command $__acre_executable __complete $token 2>/dev/null
end
complete -c acre -f -a '(__acre_complete)'
"#.to_owned(),
        SupportedShell::Powershell => r#"Register-ArgumentCompleter -Native -CommandName acre -ScriptBlock {
  param($wordToComplete)
  & $script:AcreExecutable __complete $wordToComplete 2>$null | ForEach-Object {
    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
  }
}
"#.to_owned(),
        SupportedShell::Zsh => r#"_acre_completion() {
  local -a values
  values=("${(@f)$(command "$_acre_executable" __complete "${words[CURRENT]}" 2>/dev/null)}")
  _describe 'acre target' values
}
compdef _acre_completion acre
"#.to_owned(),
        SupportedShell::Bash => r#"_acre_completion() {
  local current
  current="${COMP_WORDS[COMP_CWORD]}"
  mapfile -t COMPREPLY < <(command "$_acre_executable" __complete "$current" 2>/dev/null)
}
complete -F _acre_completion acre
"#.to_owned(),
    }
}

fn posix_integration(shell: &str) -> String {
    let completion = generate_completion(if shell == "zsh" { SupportedShell::Zsh } else { SupportedShell::Bash });
    format!(r#"# Acre shell integration for {shell}.
# Add with: eval "$(acre shell init {shell})"

_acre_executable="${{ACRE_EXECUTABLE:-$(command -v acre)}}"
if [ -z "${{ACRE_SHELL_SESSION_ID:-}}" ] && [ -n "$_acre_executable" ]; then
  export ACRE_SHELL_SESSION_ID="$(command "$_acre_executable" __session-id 2>/dev/null)"
fi

acre() {{
  local _acre_file _acre_status _acre_version _acre_action _acre_path _acre_token
  if [ -n "${{ACRE_EXECUTABLE:-}}" ] && [ -x "$ACRE_EXECUTABLE" ]; then
    _acre_executable="$ACRE_EXECUTABLE"
  fi
  if [ -z "$_acre_executable" ] || [ ! -x "$_acre_executable" ]; then
    printf '%s\n' 'Acre executable not found.' >&2
    return 127
  fi
  _acre_file="$(mktemp "${{TMPDIR:-/tmp}}/acre-directive.XXXXXX")" || return 3
  chmod 600 "$_acre_file" 2>/dev/null || true
  if ACRE_DIRECTIVE_FILE="$_acre_file" ACRE_SHELL_SESSION_ID="$ACRE_SHELL_SESSION_ID" ACRE_SHELL_PID="$$" command "$_acre_executable" "$@"; then
    _acre_status=0
  else
    _acre_status=$?
  fi

  if {{ [ "$_acre_status" -eq 0 ] || [ "$_acre_status" -eq 194 ]; }} && [ -s "$_acre_file" ]; then
    exec 9<"$_acre_file"
    IFS= read -r -d '' _acre_version <&9 || true
    IFS= read -r -d '' _acre_action <&9 || true
    IFS= read -r -d '' _acre_path <&9 || true
    IFS= read -r -d '' _acre_token <&9 || true
    exec 9<&-
    if [ "$_acre_version" != "acre-directive-v1" ]; then
      printf '%s\n' 'Acre returned an unsupported shell directive.' >&2
      _acre_status=3
    elif [ "$_acre_action" = "cd" ] && [ "$_acre_status" -eq 0 ]; then
      if [ -d "$_acre_path" ]; then
        builtin cd -- "$_acre_path" || _acre_status=$?
      else
        printf 'Acre navigation target is unavailable: %s\n' "$_acre_path" >&2
        _acre_status=3
      fi
    elif [ "$_acre_action" = "resume-after-cd" ] && [ "$_acre_status" -eq 194 ]; then
      if [ -d "$_acre_path" ] && [ -n "$_acre_token" ]; then
        builtin cd -- "$_acre_path" || _acre_status=$?
        if [ "$_acre_status" -eq 194 ]; then _acre_status=0; fi
        if [ "$_acre_status" -eq 0 ]; then
          : > "$_acre_file"
          if ACRE_DIRECTIVE_FILE="$_acre_file" ACRE_SHELL_SESSION_ID="$ACRE_SHELL_SESSION_ID" ACRE_SHELL_PID="$$" command "$_acre_executable" __resume "$_acre_token"; then
            _acre_status=0
          else
            _acre_status=$?
          fi
        fi
      else
        printf 'Acre safe destination is unavailable: %s\n' "$_acre_path" >&2
        _acre_status=3
      fi
    fi
  fi
  rm -f -- "$_acre_file"
  return "$_acre_status"
}}

{completion}"#)
}

fn fish_integration() -> String {
    let completion = generate_completion(SupportedShell::Fish);
    format!(r#"# Acre shell integration for fish.
set -g __acre_executable (test -n "$ACRE_EXECUTABLE"; and echo $ACRE_EXECUTABLE; or type -P acre)
if test -z "$ACRE_SHELL_SESSION_ID"; and test -n "$__acre_executable"
  set -gx ACRE_SHELL_SESSION_ID (command $__acre_executable __session-id 2>/dev/null)
end
function acre --description 'Warm, reusable Git workspaces'
  set -l file (mktemp /tmp/acre-directive.XXXXXX)
  or return 3
  env ACRE_DIRECTIVE_FILE=$file ACRE_SHELL_SESSION_ID=$ACRE_SHELL_SESSION_ID ACRE_SHELL_PID=$fish_pid $__acre_executable $argv
  set -l code $status
  if test -s $file; and contains -- $code 0 194
    set -l fields (string split0 < $file)
    if test "$fields[1]" = acre-directive-v1
      if test "$fields[2]" = cd; and test $code -eq 0
        builtin cd -- "$fields[3]"
        set code $status
      else if test "$fields[2]" = resume-after-cd; and test $code -eq 194
        builtin cd -- "$fields[3]"
        set code $status
        if test $code -eq 0
          env ACRE_DIRECTIVE_FILE=$file ACRE_SHELL_SESSION_ID=$ACRE_SHELL_SESSION_ID ACRE_SHELL_PID=$fish_pid $__acre_executable __resume "$fields[4]"
          set code $status
        end
      end
    end
  end
  rm -f -- $file
  return $code
end

{completion}"#)
}

fn powershell_integration() -> String {
    let completion = generate_completion(SupportedShell::Powershell);
    format!(r#"# Acre shell integration for PowerShell.
$script:AcreExecutable = if ($env:ACRE_EXECUTABLE) {{ $env:ACRE_EXECUTABLE }} else {{ (Get-Command acre -CommandType Application -ErrorAction SilentlyContinue).Source }}
if ($script:AcreExecutable -and -not $env:ACRE_SHELL_SESSION_ID) {{
  $env:ACRE_SHELL_SESSION_ID = (& $script:AcreExecutable __session-id 2>$null).Trim()
}}
function global:acre {{
  param([Parameter(ValueFromRemainingArguments = $true)][string[]]$AcreArgs)
  $file = Join-Path ([IO.Path]::GetTempPath()) ('acre-directive-' + [guid]::NewGuid().ToString('N'))
  [IO.File]::WriteAllBytes($file, [byte[]]@())
  $env:ACRE_DIRECTIVE_FILE = $file
  $env:ACRE_SHELL_PID = [string]$PID
  & $script:AcreExecutable @AcreArgs
  $code = $LASTEXITCODE
  if ((Test-Path $file) -and (Get-Item $file).Length -gt 0 -and ($code -eq 0 -or $code -eq 194)) {{
    $fields = ([Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($file))).Split([char]0, [StringSplitOptions]::RemoveEmptyEntries)
    if ($fields[0] -eq 'acre-directive-v1') {{
      if ($fields[1] -eq 'cd' -and $code -eq 0) {{ Set-Location -LiteralPath $fields[2] }}
      elseif ($fields[1] -eq 'resume-after-cd' -and $code -eq 194) {{
        Set-Location -LiteralPath $fields[2]
        & $script:AcreExecutable __resume $fields[3]
        $code = $LASTEXITCODE
      }}
    }}
  }}
  Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue
  $global:LASTEXITCODE = $code
}}

{completion}"#)
}
