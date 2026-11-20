param([Parameter(Mandatory=$true)][string]$StateDirectory)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition @'
using System.Windows.Forms;
public sealed class BackgroundButtonForm : Form {
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr value);
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override CreateParams CreateParams {
        get { var p = base.CreateParams; p.ExStyle |= 0x08000000; return p; }
    }
}
'@
[void][BackgroundButtonForm]::SetThreadDpiAwarenessContext([IntPtr](-4))
[System.Windows.Forms.Application]::EnableVisualStyles()
$form = New-Object BackgroundButtonForm
$form.Text = 'Synapse criterion 7 owned fixture'
$form.FormBorderStyle = 'None'
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(80,80)
$form.ClientSize = New-Object System.Drawing.Size(240,160)
$form.BackColor = [System.Drawing.Color]::CornflowerBlue
$button = New-Object System.Windows.Forms.Button
$button.Text = 'Press'
$button.Bounds = New-Object System.Drawing.Rectangle(60,50,120,60)
$form.Controls.Add($button)
$script:count = 0
$counterPath = Join-Path $StateDirectory 'counter.txt'
[System.IO.File]::WriteAllText($counterPath, '0')
$button.Add_Click({
    $script:count += 1
    $button.Text = "Pressed $script:count"
    $form.BackColor = [System.Drawing.Color]::Tomato
    [System.IO.File]::WriteAllText($counterPath, [string]$script:count)
})
$form.Add_Shown({
    # Publish only after the real Button HWND exists. Borderless client and
    # window coordinates coincide; its centre is (120,80).
    $ready = @{
        hwnd = $form.Handle.ToInt64()
        button_hwnd = $button.Handle.ToInt64()
        x = 120
        y = 80
    } | ConvertTo-Json -Compress
    $pending = Join-Path $StateDirectory 'ready.tmp'
    [System.IO.File]::WriteAllText($pending, $ready)
    [System.IO.File]::Move($pending, (Join-Path $StateDirectory 'ready.json'))
})
[System.Windows.Forms.Application]::Run($form)
