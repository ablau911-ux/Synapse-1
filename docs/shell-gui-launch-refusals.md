# Shell GUI launch refusals (#268)

On the visible desktop, `shell operation=run` (inline or durable) and
`shell operation=start` check physical user activity before classifying commands.
An idle user, a disabled quiet window, or approval scoped to the current call
skips classification. Otherwise, a detected GUI launch returns `USER_ACTIVE`
or `USER_ACTIVITY_UNKNOWN` before creating a child process or a durable job.
The refusal includes `detected_launch`, `delivery_state=refused_before_delivery`,
and the activity source of truth. It writes a session-owned `denied` action-audit
row, without a `started` row.

Wait for the quiet window, or use `process operation=launch` with
`owner_approved=true` and a non-empty reason for the exact requested launch.
The shell facade also accepts that explicit approval for one run/start call
and records the reason in its command audit.

The classifier recognizes launch verbs and local GUI PE executables only in
command positions. It inspects the first command, separators (`;`, `&`, `&&`,
`||`, `|`, and newlines), and both sides of parentheses and script-block braces.
It consumes cmd `if` conditions (`not`, `exist`, `defined`, `errorlevel`, and
comparisons), recognizes `else`, skips `for` headers through `do`, and follows
`call`. PowerShell `if`/`elseif`/`else`, `foreach`, `for`, `while`, `do`/`until`,
`switch`, `try`/`catch`/`finally`, `trap`, `return`, `&`, and dot-source `.` expose
their command positions. PowerShell assignment (`$p = Start-Process notepad` or
`$p=Start-Process notepad`) exposes the right-hand command; `=` embedded in an
ordinary argument such as `--format=start` does not. Inspection conservatively
recognizes both grammars inside wrappers and includes unreachable branches;
it does not evaluate conditions.

A bare quoted PowerShell literal is a value even in command position. Assignments
such as `$action = 'start'`, hashtable values, and `Write-Output ('notepad')` do
not launch anything. The `&` and `.` operators still invoke string literals,
and `$()` inside double-quoted values still receives inspection.

Token quoting follows the current evaluator. Cmd recognizes only double quotes
and uses caret (`^`) to escape characters outside quotes. Apostrophes and
backticks are ordinary cmd characters, so `echo it's & start ...` and
``for /f "usebackq" %i in (`echo 1`) do start ...`` retain their command
boundaries. PowerShell and `iex` recognize single and double quotes, with
backtick escaping outside single quotes; caret is ordinary, so `echo a^;
Start-Process ...` contains two commands. Nested interpreters select their own
quoting rules. Cmd `if` compares raw tokens, preserving empty quoted operands
in `if "%x%"=="" ...` and the optional `/i` and `not` condition prefixes.

Only evaluator payloads are parsed again: cmd `/c`, `/k`, or `/R`, PowerShell/pwsh
`-Command` (including `-c`, `-co`, and other prefixes) or the first non-switch
argument, `iex`/`Invoke-Expression` arguments, and string literals invoked with
`&` or `.`. PowerShell also evaluates balanced `$()` subexpressions inside
double-quoted strings, including nested expressions. Single-quoted strings and
backtick-escaped dollar signs remain data. Quoting is retained in ordinary
arguments, so `git commit -m 'write tests'`, `npm start`, `docker start db`, and
`net start` remain console commands.

`iex` and `Invoke-Expression`, including case variants and
`Microsoft.PowerShell.Utility\Invoke-Expression`, require one quoted string
literal without interpolation. The parser skips `-Verbose` and `-Debug`, and
consumes the values of the common action, variable, buffer, and pipeline
parameters (including their short aliases). It accepts positional literals,
`-Command <literal>`, `-Command:<literal>`, and `-Command: <literal>`.
Missing payloads or parameter values, unknown parameters, variables,
interpolated double quotes, subexpression or array payloads, concatenations,
and additional positional expressions are undeterminable and refuse while
activity is recent or unknown. Common parameters may follow the literal;
remaining tokens and later command segments still receive inspection.

Cmd payload switches are case-insensitive and may be glued to their payload:
`cmd /cstart firefox` and `cmd /c"start firefox"` receive the same inspection
as `cmd /c start firefox`. PowerShell switch prefixes `/`, en dash (U+2013),
em dash (U+2014), and horizontal bar (U+2015) are normalized to `-` before
parsing switch names and argument widths.

Known interpreter switches consume their documented argument widths; unknown
switches consume one token and scanning continues. Colon forms such as
`-ExecutionPolicy:Bypass` consume one token. In pwsh, `-i` means `-Interactive`
and consumes one token. Only `-File`/`-f` and `-EncodedCommand`/`-e`/`-ec` (and
their prefixes) stop inspection of an interpreter's payload. A standalone
info switch requires no payload: pwsh `-v`, `-Version`, or `--version`,
PowerShell/pwsh `-?` or `-Help`, and cmd `/?` are allowed. See Microsoft's
[pwsh parameters](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_pwsh)
and [Windows PowerShell parameters](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe).

Evaluator nesting is limited to four levels, including the outer shell. Deeper
nesting (including evaluated subexpressions) refuses while activity is recent or
unknown, even with a console-only leaf. The refusal names the evaluator recursion
depth limit in `detected_launch`;
idle and explicitly approved calls still skip classification.

Known interpreter wrappers and recognized `Invoke-Expression` evaluators fail
closed when inspection cannot locate or parse their payload. Missing command
payloads, missing switch values, unterminated
quotes or subexpressions, unfinished escapes, and excessive evaluator nesting
produce `could not determine whether this starts a GUI` with the reason in
`detected_launch`. They use the same activity refusal and denial audit before
any child process or durable job is created. A parse failure in a recognized
wrapper or evaluator never becomes a "not GUI" result. Explicit `-File` and
`-EncodedCommand` payloads remain documented inspection exclusions, and
standalone info switches have no payload; neither is a parser failure.

Recognized verbs are `Start-Process`, `saps`, cmd `start` (including `@start`
and attached switches such as `start/max`), `explorer`/`explorer.exe`,
`Invoke-Item`, `ii`, and `rundll32` URL handlers. Other command tokens are
resolved through PATH, PATHEXT, and App Paths and inspected for a GUI PE
subsystem without execution. A path's final segment never makes it a verb:
`C:\proj\start`, `tools/Start-Process`, and `start/run.sh` require GUI PE
resolution like any other path. The exact module-qualified cmdlets
`Microsoft.PowerShell.Management\Start-Process` and
`Microsoft.PowerShell.Management\Invoke-Item` are recognized case-insensitively.

Within PowerShell, known console aliases and cmdlets (including `write`, `echo`,
`sleep`, `cat`, `type`, `Write-Output`, `Write-Host`, `Start-Sleep`, `Get-Content`,
and `ForEach-Object`) take precedence over same-name PATH executables. Explicit
`.exe` names and executable paths still receive PE inspection. Direct shell
facade executable calls and cmd invocations do not receive this exemption.
See Microsoft's [command precedence](https://learn.microsoft.com/powershell/module/microsoft.powershell.core/about/about_command_precedence)
and [quoting rules](https://learn.microsoft.com/powershell/module/microsoft.powershell.core/about/about_quoting_rules).

This is not a shell interpreter or a complete GUI-launch boundary. Known
uncaught forms include:

- Script contents, including PowerShell `-File` and executable script wrappers.
- PowerShell `-EncodedCommand` / `-enc`, and string concatenation or variable-based
  invocation such as `& $command` outside recognized `Invoke-Expression` payloads.
- `.lnk`, `.url`, and `.html` documents opened through cmd file associations.
- `.cmd` wrappers of GUI applications, such as `code` / `code.cmd`.
- COM `Shell.Application.ShellExecute` and `Out-GridView`.
- GUI launches inside bash, WSL, Python, or Node wrappers.
- Commands evaluated inside cmd `for /f` sets (the command after `do` is inspected).
- Runtime alias/function redefinitions, session-specific or imported commands,
  and dynamically constructed module names. Builtin precedence uses a fixed
  set of known console names, not the interpreter's live command table.
- PowerShell here-string delimiters, interpolation beyond balanced `$()`
  subexpressions, and other syntax outside the token model are not fully parsed.
- UNC executable paths, including verbatim UNC and UNC App Paths entries.
  These are classified as unknown, not GUI, and are never opened for PE reads.
  Device namespaces are excluded from PE reads as well.
- Script-block and command-object invocation APIs: `[scriptblock]::Create('…').Invoke()`,
  `$ExecutionContext.InvokeCommand.InvokeScript(…)`, and `& (Get-Command x)`.
- PowerShell typographic quotes (U+2018, U+2019, U+201C, U+201D). PowerShell
  treats them as quotes; the tokenizer treats them as plain characters, so a
  launch written with smart quotes may not be detected.
- Non-canonical interpreter names such as `powershell_ise`, `cmd.` (trailing
  dot), or 8.3 short names such as `POWERS~1.EXE`. These are not recognized as
  shell wrappers.
- Redirection after an `Invoke-Expression` literal (`iex 'Get-Date' > out.txt`)
  is refused as undeterminable while the owner is active (a false positive, not
  a bypass).

Issue #282 proposes a runtime foreground watchdog that would catch focus theft
from any of these forms by behaviour rather than syntax.

Detection does not change shell authorization: an unrecognized launch still
passes through the existing command policy. Hidden-desktop process routes retain
their existing behavior.
