# DeepSeek TUI 快捷启动脚本
# 作者: Claude Code
# 创建时间: 2026-05-19

param(
    [switch]$Auto,
    [switch]$Model
)

$exePath = "$env:USERPROFILE\AppData\Roaming\npm\node_modules\deepseek-tui\bin\downloads\deepseek.exe"
$workDir = "$env:USERPROFILE\DeepSeek Tui"

# 设置环境
$env:DEEPSEEK_API_KEY = "sk-f1bccc35f03d4e90be027a54ef02399a"

# 构建参数
$args = @()
if ($Auto) { $args += "--auto" }
if ($Model) { $args += "--model", $Model }

# 启动
Set-Location $workDir
& $exePath @args