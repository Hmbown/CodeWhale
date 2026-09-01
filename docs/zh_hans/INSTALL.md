# 安装 Codewhale

> 本文翻译自英文版 [INSTALL.md](../INSTALL.md)，与英文修订 `1563ce351`（2026-08-18）同步。

本文涵盖所有受支持的安装方式，以及最常见的"没装上"失败场景，包括 **Linux ARM64** 和其他不太常见的平台。

如果你只想看精简的版本，请看[主 README](../../README.md#install) 或[简体中文 README](../../README.zh-CN.md#安装)。

本分支描述的是 **v0.9.11 源码候选版**。使用 `latest` 的安装命令会解析到最新已发布的包或 GitHub Release，这可能落后于源码候选版。候选版只有在对应的包、标签、校验和与发布资源齐备之后，才算正式发布的安装。

在 macOS 和 Linux 上，网站安装器是最短的安装/更新路径：

```bash
curl -fsSL https://codewhale.net/install.sh | sh
```

它会下载匹配的 `codewhale` 和 `codew` 发布二进制，对照 `codewhale-artifacts-sha256.txt` 校验，默认安装到 `~/.local/bin`，并暴露 `codew` 便捷命令。

---

## 1. 支持平台

已发布的 Codewhale 版本会为受支持的平台/架构组合提供配套的 `codewhale` 和 `codew` 预编译二进制。下表是 v0.9.11 候选版的预期矩阵；Android/Termux 为预览状态，等待真机 QA。Linux ARM64 自 v0.8.8 起可用。Linux RISC-V 预编译暂时暂停，因为锁定的 `rquickjs-sys` 依赖没有提供 `riscv64gc-unknown-linux-gnu` 绑定。

| 平台 | 架构 | npm install | `cargo install` | GitHub 发布资源 |
| ------------ | ------------ | :---------: | :-------------: | ----------------------------------------------------- |
| Linux | x64 (x86_64) | ✅ | ✅ | `codewhale-linux-x64`, `codew-linux-x64` |
| Linux | arm64 | ✅ | ✅ | `codewhale-linux-arm64`, `codew-linux-arm64` |
| Android / Termux | arm64 (aarch64) | ⚠️⁴ 预览版 | ⚠️⁴ 预览版 | `codewhale-android-arm64.tar.gz` 发布时的预览压缩包 |
| Linux | riscv64 | ❌¹ | ❌³ | 暂时不支持，待上游绑定落地 |
| macOS | x64 | ✅ | ✅ | `codewhale-macos-x64`, `codew-macos-x64` |
| macOS | arm64 (M 系列) | ✅ | ✅ | `codewhale-macos-arm64`, `codew-macos-arm64` |
| Windows | x64 | ✅ | ✅ | `codewhale-windows-x64.exe`, `codew-windows-x64.exe` |
| Windows | arm64 | ✅ | ✅ | `codewhale-windows-arm64.exe`, `codew-windows-arm64.exe` |
| Linux x64 或 arm64 上的 musl（Alpine） | 原生架构 | ✅（静态） | ✅ | 匹配的静态 Linux 资源 |
| 其他 Linux（其他架构上的 musl） | — | ❌¹ | ✅² | 从源码构建 |
| FreeBSD 14+ / OpenBSD | x64, arm64 | ❌ | ✅² | `cargo install codewhale-cli --locked`（无预编译；见 § FreeBSD） |

¹ npm 包会以明确错误退出，并引导你到这里。
² 前提是你的工具链能编译较新的 Rust workspace；见下文[从源码构建](#7-从源码构建)。
³ RISC-V 源码构建目前需要上游 `rquickjs-sys` 的 RISC-V 绑定，或启用 bindgen 的依赖构建。
⁴ v0.9.11 源码候选版的 npm 包装器能识别 Android arm64，并解析匹配的 `codewhale` 和 `codew` Android 资源。npm 安装仅对 GitHub Release 已发布的、匹配的包版本有效。在 #4236 和 #4242 跟踪的真机编译、启动、审批、文件工具与更新检查完成之前，Android/Termux 路径仍为预览。

Android / Termux 与 Linux arm64 不是同一个目标。不要在 Termux 里安装 Linux 的 `codewhale-linux-arm64` 压缩包；当某个发布版或候选版发布了 Termux 专用的 Android 压缩包时请使用它，或在 Termux 内从源码构建。

Linux 的 **x64 和 arm64** v0.9.11 候选版资源是**静态 musl 构建**。x64 发布路径自 v0.8.65 起使用 musl；v0.9.6 将同样的构建与静态启动检查扩展到 arm64。这些二进制没有 glibc 依赖，可在匹配的架构上跨 Ubuntu、Debian、RHEL/CentOS 和 Alpine/musl 运行。SQLite 通过 `rusqlite` 内置，因此无需单独的 `libsqlite3` 运行时包。

### Linux ARM64 可移植性

v0.9.6 之前的 Linux arm64 资源是 GNU libc 构建，可能继承了 Ubuntu 24.04 构建主机的 `GLIBC_2.39` 最低要求。
Ubuntu 22.04 自带 glibc 2.35，因此，那些较老的 arm64 二进制可能报错，例如：

```text
version `GLIBC_2.39' not found
```

npm 包装器、`codewhale update` 和 Unix 压缩包安装器对较旧版本仍保留 GNU 二进制预检查。v0.9.11 arm64 候选版改用 `aarch64-unknown-linux-musl`，因此没有 `GLIBC_*` 最低要求。如果你要在较旧的 arm64 发行版上安装早期版本，请使用：

```bash
cargo install codewhale-cli --locked   # 安装 codewhale
```

> **Linux ARM64 说明（v0.8.7 及更早）。** v0.8.7 及更早版本**未发布** Linux ARM64 预编译；
> 使用HarmonyOS 轻薄本、Asahi Linux、树莓派(Raspberry Pi)、AWS Graviton 等的用户会从 `npm i -g codewhale` 看到 `Unsupported architecture: arm64`。
> v0.8.8 发布了 `codewhale-linux-arm64`，因此普通的 `npm i -g codewhale` 可在任何基于 glibc 的 ARM64 Linux 上工作。
> 如果你还卡在 v0.8.7，直接跳到[从源码构建](#7-从源码构建)——`cargo install` 完全可用。
> HarmonyOS PC 与 OpenHarmony 交叉构建设置，见 [HarmonyOS 与 OpenHarmony](../HarmonyOS.md)。

### Android / Termux arm64

Termux 运行在 Android 的 Bionic libc 上，并使用 `$PREFIX` 作为其 Unix 前缀，因此需要 Termux 专用的 Android arm64 压缩包。Linux arm64 发布资源面向标准 Linux（使用 musl），而 Android 使用不同的 Rust 目标。因此，不应在那里使用 Linux 资源。

先安装最基本的压缩包/运行时工具：

```bash
pkg update
pkg install -y ca-certificates curl tar gzip coreutils
```

当发布版包含 `codewhale-android-arm64.tar.gz` 时，用压缩包自带的安装器安装。传入 `PREFIX="$PREFIX"` 很重要：安装器默认安装到 `~/.local`，而 Termux 用户通常期望命令在 `$PREFIX/bin` 下。

```bash
cd "$HOME"
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-android-arm64.tar.gz
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-bundles-sha256.txt
sha256sum -c codewhale-bundles-sha256.txt --ignore-missing

tar xzf codewhale-android-arm64.tar.gz
cd codewhale-android-arm64
PREFIX="$PREFIX" ./install.sh
hash -r
```

如果你要验证源码或在本地构建候选版，请在运行 Cargo 之前，先安装构建包：

```bash
pkg install -y rust clang pkg-config make git
cargo install codewhale-cli --locked   # 安装 codewhale
```

正确的首次运行设置流程已实现，但其 Android 交互仍属于上文提到的预览 QA 范围。
临时凭证优先使用 provider 环境变量。
`codewhale auth set` 可用，但 Termux 构建没有受支持的 OS 钥匙串集成（keyring integration），会退化为文件存储的密钥：写入 `~/.codewhale/config.toml`，并把密钥镜像到 `~/.codewhale/secrets/secrets.json`。两者都是纯文本文件，受 `0600` 权限保护，静态存储时未加密。

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
```

维护者应对 Termux / Android arm64 候选版使用这套可重复的冒烟检查清单（smoke checklist）：

```bash
command -v codewhale codew
test -x "$PREFIX/bin/codewhale"
test -x "$PREFIX/bin/codew"

codewhale --version
codewhale doctor
codewhale exec --auto "run pwd"
```

已知限制：

- 命令会继承 Android 的每应用（per-app） UID、SELinux 和 seccomp 保护，以及授予 Termux 的任何权限。Codewhale 的可选 bubblewrap 子进程沙箱仅限 Linux，未在 Android 上构建，因此已批准的命令不会获得 Codewhale 特有的文件系统限制。
- Termux 构建没有受支持的 Android Keystore 或桌面 Secret Service 集成。用 `codewhale auth status` 确认当前生效的来源；当文件型纯文本存储不可接受时，优先使用 provider 环境变量。
- 终端渲染因 Android 终端应用而异。TUI 始终拥有备用屏幕（alternate screen）。如果某个终端应用无法渲染全屏 TUI，请改用 `codewhale exec` 来无头运行。

---

## 2. 下载安全与校验和

官方发布二进制只从 `https://github.com/Hmbown/CodeWhale/releases` 和名为 `codewhale` 的 npm 包发布。除非你明确信任某个镜像，请勿从仿冒的仓库、压缩包和搜索结果镜像安装发布资源。

每个 GitHub release 都包含校验和清单。使用 `codewhale-artifacts-sha256.txt` 校验裸二进制文件，使用 `codewhale-bundles-sha256.txt` 校验 `.tar.gz` / `.zip` 平台压缩包。如果您手动下载二进制文件，请在运行前进行验证：

```bash
# 在包含已下载的二进制的目录中运行。
curl -L -O https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
sha256sum -c codewhale-artifacts-sha256.txt --ignore-missing
```

在 macOS 上，用 `shasum -a 256 -c codewhale-artifacts-sha256.txt --ignore-missing` 来代替 `sha256sum`。

如果杀毒软件标记了官方发布的二进制文件，请在找出确切的工件（Artifact）之前将其视为未解决的问题。请在 GitHub issue 中提供以下所有信息：

- 发布标签，例如 `v0.8.36`
- 确切的下载 URL
- 文件名，例如 `codewhale-linux-x64`
- 你机器上文件的 SHA-256
- 杀毒软件产品名与检测名称

这能让维护者区分官方工件的误报与来自仿冒仓库或镜像的下载。

---

## 3. 通过 npm 安装

npm 是推荐的安装方式（Node 18+；包装器适用于 v0.8.56 及更高版本）。它安装的是 注册表（registry） 上最新发布的版本，而不是未发布的源码候选版。

```bash
npm install -g codewhale
codewhale --version   # 打印已安装的发布版本
```

`postinstall` 会下载匹配的 `codewhale` 和 `codew` 二进制，对照该来源的 SHA-256 清单校验，并把 `codewhale` 和 `codew` 添加到你的 `PATH` 中。

在 **Linux x64**（包括 OpenHarmony x64）上，包装器**不会**等待缓慢的 GitHub 二进制下载或漫长的超时失败。
除非你设置了显式的 release 基础 URL 或 `CODEWHALE_USE_CNB_MIRROR=1`，否则它会并发地从 GitHub Releases 和第一方 CNB release 获取对应精确包版本的小型 `codewhale-artifacts-sha256.txt` 清单，接受第一个对所需资源通过 HTTP 响应与清单校验的来源，取消另一个探测，并且只从该锁定来源下载二进制。
CNB 只发布 Linux x64；其他目标保持仅 GitHub 路径。所选来源会打印在安装进度中，并写入下载文件旁的 `<binary>.source`。校验和（checksum）或来源不匹配时按失败处理。

在 Windows 上，请从 **Windows Terminal** 运行这些命令，而不是 `cmd.exe`，这样字体和颜色才能匹配受支持的 TUI。GitHub Release 还会在裸 x64 exe 旁发布 `codewhale.bat`；该启动器优先使用 `wt.exe`，在没有 Windows Terminal 时回退为直接启动。

有用的环境变量：

| 变量 | 用途 |
| ----------------------------------- | -------------------------------------------------------------------------------------- |
| `CODEWHALE_RELEASE_BASE_URL` | 覆盖下载根目录。跳过 Linux x64 的 GitHub/CNB 竞争。 |
| `CODEWHALE_USE_CNB_MIRROR=1` | 在 Linux x64 / OpenHarmony x64 上强制使用 CNB 第一方镜像。其他目标会失败。 |
| `CODEWHALE_VERSION` | 固定包装器下载哪个 release（默认 `codewhaleBinaryVersion`）。 |
| `CODEWHALE_GITHUB_REPO` | 让下载器指向某个 fork（`owner/repo`）。 |
| `CODEWHALE_FORCE_DOWNLOAD=1` | 即使缓存的二进制标记匹配也重新下载。 |
| `CODEWHALE_DISABLE_INSTALL=1` | 完全跳过 `postinstall` 下载（CI 冒烟、内置二进制）。 |
| `CODEWHALE_OPTIONAL_INSTALL=1` | 遇到可重试的下载错误时不使 `npm install` 失败——在 CI 矩阵中有用。 |
| `CODEWHALE_QUIET_INSTALL=1` | 禁止安装器进度消息，静默安装。 |
| `CODEWHALE_DOWNLOAD_TIMEOUT_MS` | 覆盖总下载超时时间（毫秒）。 |
| `CODEWHALE_DOWNLOAD_STALL_MS` | 覆盖无进度停滞超时时间（毫秒）。 |

相应的 `DEEPSEEK_TUI_*` 和 `DEEPSEEK_*` 变量仍作为旧别名被接受，但规范的名称是 `CODEWHALE_*`。新的自动化与支持文档应只使用 `Codewhale` 名称。

> **中国大陆 npm 下载慢？** 如果 `npm install` 本身很慢（不只是 postinstall 的二进制下载），使用 npm 注册表镜像：
> ```bash
> npm config set registry https://registry.npmmirror.com
> npm install -g codewhale
> ```
> 如果你更想用 Cargo 而非 npm，参见[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)。

---

## 4. 通过 Cargo 安装（任何 Tier-1 Rust 目标）

如果 GitHub releases 缓慢、受阻，或你正在使用不受支持的架构，可以直接从 crates.io 安装。
只需要一个 Cargo 包：`codewhale-cli` 会安装 `codewhale` 命令。npm 与预编译发布版还会把 `codew` 作为同一编译运行时的便捷名称暴露出来；Cargo 不会创建该别名，所以如果你想要更短的名字，请自行定义 shell 别名。

```bash
# 需要 Rust 1.88+（https://rustup.rs）
cargo install codewhale-cli --locked   # 安装 codewhale
codewhale --version
```

> **Linux：先安装构建时依赖。** `cargo install` 从源码编译，在 Linux 上 `codewhale-cli` crate 会链接 `libdbus-1`（D-Bus secret-service 后端用它存储凭据）。运行 `cargo install` 之前请先安装所需的系统包：
>
> ```bash
> # Debian / Ubuntu
> sudo apt-get install -y build-essential pkg-config libdbus-1-dev
>
> # Fedora / RHEL
> sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel
> ```
>
> 如果你使用 npm 包装器或下载 GitHub Release 二进制，这些构建时包就**不需要**了——预编译二进制只需要运行时库（`libdbus-1`），而大多数桌面 Linux 安装里已经自带。

### 中国/镜像友好安装

从中国大陆安装时，请同时为 **rustup**（Rust 工具链安装器）和 **Cargo**（包注册表）配置镜像，以避免 TLS 超时和下载失败。

**第 1 步：通过 rustup 镜像安装 Rust**

```bash
# PowerShell
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
(New-Object Net.WebClient).DownloadFile('https://win.rustup.rs/x86_64', 'rustup-init.exe')

# git-bash / msys2
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
./rustup-init.exe -y --default-toolchain stable

# Linux / macOS
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
```

如果 TUNA 镜像在你的网络下很慢，`rsproxy.cn` 是 Linux/macOS 的另一个 rustup 镜像选择：

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
```

`RUSTUP_DIST_SERVER` 和 `RUSTUP_UPDATE_ROOT` 环境变量**必须**在运行 rustup-init **之前**设置；否则工具链下载会遇到与安装器相同的 TLS 握手问题。

**第 2 步：配置 Cargo registry 镜像**

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

`rsproxy`、腾讯云 COS 和阿里云 OSS 镜像的工作方式相同；根据你的网络环境选择最快的即可。

## 5. 通过 Nix 安装

**试试看**

如果你已经有支持 flake 的 Nix，运行：

```sh
nix run github:Hmbown/CodeWhale
```

Nix 会构建 `codewhale`（单个二进制），然后启动调度器。在 `--` 之后传参，例如：

```sh
nix run github:Hmbown/CodeWhale -- --help
```

### Flake

在 `flake.nix` 中添加 inputs：

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    codewhale.url = "github:Hmbown/CodeWhale";
    codewhale.inputs.nixpkgs.follows = "nixpkgs";
  };
}
```

安装到 NixOS 模块中：

```nix
{
  outputs = { self, nixpkgs, codewhale }:
  let
    # 把 system "x86_64-linux" 替换成你的系统
    system = "x86_64-linux";
  in
  {
    # 把 `yourhostname` 改成你的真实主机名（Hostname）
    nixosConfigurations.yourhostname = nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        # ...
        {
          environment.systemPackages = [ codewhale.packages.${system}.default ];
        }
      ];
    };
  };
}
```

---

## Omarchy / AUR

在 Omarchy 上安装预构建的 AUR 包：

```bash
omarchy pkg aur add codewhale-bin
codewhale --version
```

`codewhale-bin` 打包与其他二进制安装路径相同的、经校验和固定的 Linux 发布压缩包，并提供 `codewhale` 和 `codew` 两个命令。它不携带单独的 Codewhale 版本；现有的 `codewhale-tui` 兼容命令仍是同一运行时的别名。包更新通过 `omarchy update` 到达；应用内更新器会把 pacman 拥有的二进制留给 Omarchy。

AUR 更新跟随匹配的 Codewhale 标签和发布资源，因此它可能在 GitHub 发布之后才出现——其生成的 `PKGBUILD` 和 `.SRCINFO` 需要先经过验证。发布维护者说明见 [`packaging/aur/README.md`](../../packaging/aur/README.md)。

---

## Homebrew

formula 名为 `codewhale`。
tap GitHub 仓库在改名之前仍是 `Hmbown/homebrew-deepseek-tui`；`brew tap Hmbown/deepseek-tui` 无论哪种情况都能继续工作。

```bash
brew tap Hmbown/deepseek-tui
brew install codewhale
```

用 `brew upgrade codewhale` 更新。
旧的 `deepseek-tui` formula 名下的 Cellar 安装，在一个重叠发布周期内，仍可运行 `brew upgrade deepseek-tui`；新安装应使用 `codewhale`。

---

## 6. 从 GitHub Releases 手动下载

每个平台在 Releases 页面以**两种形式**出现（这是有意为之——见 #3208）：**裸二进制**（`codewhale-<platform>` 和 `codew-<platform>`，无扩展名）和 **`.tar.gz` / `.zip` 压缩包**（`codewhale-<platform>.tar.gz`），压缩包包含了同样的命令，还外加了 `install.sh`。
npm 包装器和应用内 `codewhale update` 会下载匹配的运行时二进制；压缩包是最简单的手动安装方式（见[第 6 节](#6-从-github-releases-手动下载)）。下面的步骤直接使用裸二进制文件。

从 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases)抓取匹配你平台的命令组，并把它们并排放入 `PATH` 上的某个目录（例如 `~/.local/bin`）：

```bash
# Linux ARM64 示例
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/codewhale      \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-linux-arm64
curl -L -o ~/.local/bin/codew          \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codew-linux-arm64
chmod +x ~/.local/bin/codewhale ~/.local/bin/codew
codewhale --version
```

> **macOS Gatekeeper 说明。** 如果你用浏览器下载了二进制，macOS 可能会用"Apple 无法验证"警告拦截它们。清除两个二进制的隔离属性后重试：
> ```bash
> xattr -d com.apple.quarantine ~/.local/bin/codewhale ~/.local/bin/codew 2>/dev/null || true
> ```

根据每个版本的 SHA-256 清单验证完整性：

```bash
curl -L -o /tmp/codewhale-artifacts-sha256.txt \
    https://github.com/Hmbown/CodeWhale/releases/latest/download/codewhale-artifacts-sha256.txt
( cd ~/.local/bin && sha256sum -c /tmp/codewhale-artifacts-sha256.txt --ignore-missing )
```

（在 macOS 上使用 `shasum -a 256 -c /tmp/codewhale-artifacts-sha256.txt --ignore-missing` 代替 `sha256sum -c`。）

### 回滚到之前的版本

如果某个新 release 在你的机器上出问题，请显式安装最后一个已知正常的版本。把 `X.Y.Z` 替换成你要恢复的版本。

```bash
# npm 包装器，仅对已发布到 npm 的版本有效
npm install -g codewhale@X.Y.Z

# Cargo 路径：一个包安装 codewhale
cargo install codewhale-cli --version X.Y.Z --locked --force
```

手动安装时，请从确切的 release 标签下载匹配的二进制或平台压缩包，并从同一标签校验对应的校验和清单：

```bash
# 单独的二进制
curl -L -o codewhale-artifacts-sha256.txt \
  https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-artifacts-sha256.txt

# 平台压缩包
curl -L -o codewhale-bundles-sha256.txt \
  https://github.com/Hmbown/CodeWhale/releases/download/vX.Y.Z/codewhale-bundles-sha256.txt
```

在 Codewhale 工作区内，`/restore list [N]` 列出 side-git 文件快照，`/restore <N>` 从所选快照恢复文件。这种工作区回滚不会改变你已安装的二进制版本，也不会重写对话历史。

### Windows Scoop

`codewhale` 包列在 Scoop 的 main bucket 中：

```powershell
scoop update
scoop install codewhale
codewhale --version
```

Scoop 清单维护在本仓库的发布工作流之外，可能落后于 GitHub/npm/Cargo 发布。当你需要立即拿到最新版本时，请使用 npm 或手动在 GitHub release 下载。

### Windows winget（v0.9.5+）

Codewhale 为 `Hmbown.CodeWhale` 发布 winget manifest（解决 #1561）。Winget 只安装 `codewhale` + `codew` 命令。GitHub Releases 保留字节完全一致的 `codewhale-tui-*` 文件名，仅用于旧版更新器兼容；它们不是第三个已安装命令。

```powershell
winget install Hmbown.CodeWhale
codewhale --version
```

清单位于 [`packaging/winget/Hmbown.CodeWhale.yaml`](../../packaging/winget/Hmbown.CodeWhale.yaml)（也在 [`.winget/Hmbown.CodeWhale.yaml`](../../.winget/Hmbown.CodeWhale.yaml) 镜像了一份），列出 NSIS 安装器（`CodeWhaleSetup.exe`，每用户安装，把 `%LOCALAPPDATA%\Programs\CodeWhale\bin` 加入用户 PATH）和便携 ZIP 备选（`codewhale-windows-x64.zip` / `codewhale-windows-arm64.zip`）。
winget 会自动选择匹配的架构；两者都安装单二进制文件（`codewhale.exe` + `codew.exe`）。ZIP 里还包含 `codewhale.bat`。请双击那个启动器（而不是原始 `.exe`），如此，首选窗口被设置为 Windows Terminal（如果已安装）。

通过 `winget upgrade Hmbown.CodeWhale` 或 `codewhale update` 更新。winget 包维护在本仓库的发布工作流之外，可能会比 GitHub/npm/Cargo 发布滞后一个验证周期 —— 当您需要最新版本时，请使用 npm 或 GitHub Release 资源。
如果 `winget install` 报告哈希不匹配，请校验同一标签的 `codewhale-artifacts-sha256.txt`，并通过 `packaging/winget/generate-winget-manifest.sh` 重新生成清单（见 [`packaging/winget/README.md`](../../packaging/winget/README.md)），然后重新提交到 [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)。

> **Windows ARM64 说明。** NSIS 安装器目前只包含 x64 二进制。Windows ARM64 用户应通过 `winget install Hmbown.CodeWhale`（ARM64 ZIP）或原生 ARM64 Node.js 下的 `npm install -g codewhale` 安装，或直接下载 `codewhale-windows-arm64.zip`——所有路径都会安装原生 ARM64 二进制。

### Windows NSIS 安装器

从 v0.8.50 开始，为喜欢传统双击安装的 Windows 用户提供了独立的基于 NSIS 的安装器（无需 npm、Scoop 或 Cargo）。

NSIS 安装器目前包含 Windows x64 二进制。Windows ARM64 用户应通过原生 ARM64 Node.js 下的 npm 安装，或从同一 release 下载 `codewhale-windows-arm64.zip`；两条路径都会使用原生 ARM64 二进制。

**下载** 从 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases/latest) 下载 `CodeWhaleSetup.exe`。

**安装** 双击安装程序。安装器会：

- 把 `codewhale.exe` 和 `codew.exe` 并排安装（单二进制，没有 `codewhale-tui.exe`）到 `%LOCALAPPDATA%\Programs\CodeWhale\bin`
- 安装 `codewhale.bat`，它在 `PATH` 上存在 Windows Terminal（`wt.exe`）时优先使用，否则直接启动 exe
- 创建当前用户的开始菜单快捷方式，指向该启动器，而非裸 `.exe`
- 把安装目录加入**当前用户**的 `PATH`
- 在 Windows **应用和功能（Apps & Features）** 中注册，便于卸载

卸载会移除二进制、`codewhale.bat`、开始菜单快捷方式和用户 `PATH` 条目。

**静默安装**（供 IT 管理员、SCCM、Intune 使用）：

```powershell
CodeWhaleSetup.exe /S
```

安装器是每用户安装，不会请求提权。请在目标用户的环境中运行静默安装，或使用能为每个需要 Codewhale 的用户配置文件运行安装器的部署工具。

发布版安装器目前未签名，可能触发 Windows SmartScreen。部署前请用 `codewhale-artifacts-sha256.txt` 校验 SHA-256 校验和（checksum）；如果你的环境要求签名应用包，请在内部部署管道中对安装程序进行签名。

**自行构建安装程序**（需要 [NSIS](https://nsis.sourceforge.io)）：

```powershell
cd scripts\installer
# 把 codewhale.exe 和 codew.exe 放到这里（单二进制，没有 codewhale-tui.exe），然后：
makensis /DVERSION=<version> codewhale.nsi
```

**手动回退**——如果安装器被组策略阻止，参见 [CLASSROOM_INSTALL.md](../CLASSROOM_INSTALL.md) 指南中的分步 PowerShell 命令。

> **要部署到教室或实验室？** 参见完整的[教室安装清单](../CLASSROOM_INSTALL.md)，涵盖静默安装、API key 供应、镜像说明与故障排查。

---

## 7. 从源码构建

这是面向我们不提供二进制平台的兜底方案，包括 musl 非 x64、LoongArch、FreeBSD 以及 2024 年以前的 ARM64 发行版。Linux RISC-V 目前也需要上游 `rquickjs-sys` 的 RISC-V 绑定或启用 bindgen 的依赖构建，源码构建才能预期可用。

### 前置条件

- **Rust** 1.88 或更高版本——用 [rustup](https://rustup.rs) 安装。
- **Linux 构建期依赖**（Debian/Ubuntu/openEuler/Kylin）：
  ```bash
  sudo apt-get install -y build-essential pkg-config libdbus-1-dev
  # openEuler / RHEL 系列：
  # sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel
  ```
- 不需要 `cmake`。

### 构建并安装

```bash
git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # 安装 codewhale

codewhale --version
```

命令默认安装到 `~/.cargo/bin/`；请确保该目录在你的 `PATH` 上。

### FreeBSD 14+（解决 #1097）

FreeBSD 没有预编译的 GitHub Release 资源——`npm install -g codewhale` 会故意失败，提示 `Unsupported platform: freebsd` 并指向 Cargo。从源码安装：

```bash
pkg install -y rust pkgconf git
cargo install codewhale-cli --locked   # 安装 codewhale
codewhale --version
codewhale doctor
```

`rquickjs` 的 FreeBSD 绑定在构建时通过 `bindgen` 生成（见 `1582ba965`/`5eb0385e8`）。
目前还没有单独的 `pkg install codewhale` 端口——原生端口作为 #1097 的后续工作记录在 `packaging/freebsd/` 下（欢迎贡献）。请在 release 分支上用 `cargo check --target x86_64-unknown-freebsd -p codewhale-cli --locked` 验证；7×1 发布矩阵（Linux musl x64/arm64、Android arm64、macOS x64/arm64、Windows x64/arm64）仍是 7 个目标——FreeBSD 是源码构建目标，不是预编译资源。

### 从 x64 交叉编译到 ARM64 Linux

release 资源使用 `aarch64-unknown-linux-musl`，并在原生 ARM runner 上构建。如果你想在 x64 Linux 主机上构建 GNU 链接的 ARM64 Linux 二进制（例如用于 HarmonyOS / openEuler ARM64 轻薄本），请使用 [`cross`](https://github.com/cross-rs/cross)，它把官方的 Rust 交叉目标封装在 Docker 容器中：

```bash
# 一次性
rustup target add aarch64-unknown-linux-gnu
cargo install cross --locked

# 每次构建
cross build --release --target aarch64-unknown-linux-gnu -p codewhale-cli   # 单二进制
```

生成的二进制位于 `target/aarch64-unknown-linux-gnu/release/codewhale`。把它复制到 ARM64 主机（例如通过 `scp`）并赋予可执行权限。这个本地 GNU 构建与可移植的 musl release 资源不同；两个可执行文件都可以复制到 `codew` 便捷名称下使用。

如果你没有 Docker，直接安装交叉链接器，让 Cargo 完成工作：

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu

cat >> ~/.cargo/config.toml <<'EOF'
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
EOF

cargo build --release --target aarch64-unknown-linux-gnu -p codewhale-cli   # 单二进制
```

交叉编译时生成 `aarch64-unknown-linux-musl` 需要合适的 musl 交叉链接器。release 工作流通过在 GitHub 的原生 ARM runner 上构建并启动 musl 二进制来避免这个额外的活动部件。

### Windows 源码构建

在 Windows 上构建需要 [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022) 中的 **MSVC C 工具链**（免费的可选工作负载安装器，不是完整 IDE）。

**前置条件（Windows）**

1. 安装 Visual Studio 2022 Build Tools——选择 **"使用 C++ 的桌面开发（Desktop development with C++）"** 工作负载。
2. 安装 [Rust](https://rustup.rs) 1.88+（如果从中国大陆下载，参见上文[中国/镜像友好安装](#中国镜像友好安装)）。
3. 安装 [Git for Windows](https://git-scm.com/download/win)（提供 `git` 和 `git-bash` 终端）。

**推荐的终端**：Windows Terminal、`git-bash` 或 PowerShell。`cmd.exe` 可用，但缓冲区较小且 PATH 行为有限。

**设置 MSVC 环境**

Visual Studio Build Tools 会把 `cl.exe` 安装到带版本号的目录，但**不会**把它全局加入 `PATH`。你必须手动设置环境，或使用开发者命令提示符。所需的变量是：

```powershell
# 调整版本号以匹配你的安装
$msvc = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207"
$sdk   = "C:\Program Files (x86)\Windows Kits\10"
$sdkv  = "10.0.26100.0"

$env:INCLUDE  = "$msvc\include;$msvc\atlmfc\include;$sdk\Include\$sdkv\ucrt;$sdk\Include\$sdkv\um;$sdk\Include\$sdkv\shared"
$env:LIB      = "$msvc\lib\x64;$msvc\atlmfc\lib\x64;$sdk\Lib\$sdkv\ucrt\x64;$sdk\Lib\$sdkv\um\x64"
$env:LIBPATH  = "$msvc\lib\x64;$msvc\atlmfc\lib\x64"
$env:CC       = "$msvc\bin\Hostx64\x64\cl.exe"
$env:CXX      = "$msvc\bin\Hostx64\x64\cl.exe"
$env:PATH     = "$msvc\bin\Hostx64\x64;$env:PATH"
```

或者，打开 **"VS 2022 开发者命令提示符（Developer Command Prompt for VS 2022）"**（安装 Build Tools 后可从开始菜单找到），它会运行 `vcvars64.bat` 自动配置上述所有内容。然后在该会话中把 `cargo` 加入 `PATH`，并从项目根目录运行 `cargo build`。

**Cargo registry 镜像**——在 Windows 上，镜像配置放在 `%USERPROFILE%\.cargo\config.toml`。参见[上文第 2 步](#中国镜像友好安装)。

**构建**

```bash
git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale
set CARGO_HTTP_CHECK_REVOKE=false   # 某些中国 ISP 后面可能需要
cargo build --release
```

Cargo 构建的二进制出现在 `target\release\codewhale.exe`。发布打包会另外把同一可执行文件暴露为 `codew.exe`。

> 不想构建？通过 npm、Cargo、GitHub Releases 或 CNB 镜像安装——参见上文各节。

---

## 8. Shell 补全

Codewhale 生成自己的补全脚本。每个 shell 一条命令；每个脚本同时补全 **`codewhale`** 和 `codew` 缩写。

```bash
codewhale completion <bash|zsh|fish|powershell|elvish>
```

`codewhale completions` 是同一命令的可用别名。

脚本写到 stdout，因此安装它就是把输出重定向到你的 shell 加载补全的位置。

**Bash** —— 需要你的 shell 已加载 `bash-completion` 包：

```bash
mkdir -p ~/.local/share/bash-completion/completions
codewhale completion bash > ~/.local/share/bash-completion/completions/codewhale
```

仅当前 shell 生效：`source <(codewhale completion bash)`。

**Zsh** —— 脚本的 `#compdef` 行已覆盖两个命令名：

```bash
mkdir -p ~/.zfunc
codewhale completion zsh > ~/.zfunc/_codewhale
```

如果 `~/.zfunc` 不在 `fpath` 上，请把它加入 `~/.zshrc`：

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

**Fish**：

```fish
mkdir -p ~/.config/fish/completions
codewhale completion fish > ~/.config/fish/completions/codewhale.fish
```

**PowerShell** —— 追加到你的 profile，使其在每个会话中加载：

```powershell
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $PROFILE)
codewhale completion powershell >> $PROFILE
```

仅当前会话生效：

```powershell
codewhale completion powershell | Out-String | Invoke-Expression
```

**Elvish** —— 脚本注册两个命令名：

```elvish
codewhale completion elvish >> ~/.config/elvish/rc.elv
```

升级 Codewhale 后重新生成脚本——它是生成它的那个版本的命令面快照，不是实时查询。

> 从 v0.9.10 或更早版本升级？那些版本生成的脚本注册的是内部 `codewhale-tui` 可执行文件，因此 `codewhale` 或 `codew` 没有任何补全（[#5526](https://github.com/Hmbown/CodeWhale/issues/5526)）。删除旧文件并用上面的命令重新生成。

---

## 9. 故障排查

### `Unsupported architecture: arm64 on platform linux`

你处于 v0.8.8 之前的版本，该版本不发布 Linux ARM64 二进制。要么升级（`npm i -g codewhale@latest`），要么按[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)使用 `cargo install`。

### 升级旧安装后出现 `MISSING_COMPANION_BINARY`

当前的单二进制在进程内运行 TUI，不需要配套可执行文件。该错误标识的是过时的 v0.9.5 之前调度器；请用当前的 npm 包或 Cargo 二进制替换该安装，而不是下载额外的运行时：

```bash
npm install -g codewhale
# 或
cargo install codewhale-cli --locked --force
```

### `codewhale update` 报告 `no asset found for platform codewhale-linux-aarch64`

这是 v0.8.7 中的 [#503](https://github.com/Hmbown/CodeWhale/issues/503)——自更新器使用了 Rust 的 `aarch64`/`x86_64` 架构名，而不是发布工件的 `arm64`/`x64`。v0.8.8 之前的临时方案：

```bash
npm i -g codewhale@latest
# 或
cargo install codewhale-cli --locked
```

### 中国大陆 npm 下载慢或超时

在 Linux x64 上，npm 包装器已经并行探测 GitHub Releases 和 CNB 第一方校验和清单，并且只从第一个通过校验的来源下载二进制。这条自动路径不需要 `CODEWHALE_USE_CNB_MIRROR=1`。

如果两个第一方来源都失败，把 `CODEWHALE_RELEASE_BASE_URL` 设置为镜像的 release 资源目录（rsproxy、TUNA、腾讯云 COS、阿里云 OSS），或者完全跳过 npm，使用[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)的 Cargo 镜像设置。旧的 `DEEPSEEK_TUI_RELEASE_BASE_URL` 名称仍被接受。`CODEWHALE_USE_CNB_MIRROR=1` 仍只在 Linux x64 / OpenHarmony x64 上强制 CNB。

### 中国大陆 无法从 GitHub 使用 `codewhale update`

`codewhale update` 通常会联系 GitHub Releases 获取元数据和二进制资源。在 GitHub 被屏蔽或不稳定的网络上，改用 CNB 源镜像，并从 release 标签安装 `codewhale-cli` 包。Cargo 会安装 `codewhale` 命令：

要查看最新 release 而不下载或替换二进制，运行 `codewhale update --check`。

```bash
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag vX.Y.Z codewhale-cli --locked --force   # 单二进制
```

如果你运营二进制资源镜像，`codewhale update` 可以直接使用它：

```bash
CODEWHALE_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/vX.Y.Z/ \
CODEWHALE_VERSION=X.Y.Z \
codewhale update
```

镜像目录必须包含 `codewhale-artifacts-sha256.txt` 和来自 GitHub release 的平台二进制。旧的 `DEEPSEEK_TUI_RELEASE_BASE_URL` 镜像变量仍作为别名受支持。

### Debian/Ubuntu：`cargo install` 报 `feature edition2024 is required`

一些 Debian/Ubuntu 发行版包自带较旧的 Cargo，无法解析 Rust 2024 crate。例如，Ubuntu 24.04 上的 Cargo 1.75.0 会在构建前失败，报错：

```text
feature `edition2024` is required
The package requires the Cargo feature called `edition2024`, but that feature
is not stabilized in this version of Cargo
```

通过 rustup 安装当前的 stable Rust，然后重新运行[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)中的那条 Cargo 包安装命令。它会安装 `codewhale`。对于中国大陆网络，以下基于 rsproxy 的序列已验证可用：

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn
export RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup default stable
cargo install codewhale-cli --locked   # 安装 codewhale
```

之后，`which cargo` 应指向 `~/.cargo/bin/cargo`，而不是 `/usr/bin/cargo`。

### Debian/Ubuntu：构建时报 `error: linker 'cc' not found`

安装 C 工具链：

```bash
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
```

### WSL2 / Ubuntu：构建时找不到 `dbus-1` 或 `pkg-config`

WSL2 与 Ubuntu 使用相同的 Linux 源码构建路径。如果 `cargo install codewhale-cli --locked` 在编译 keyring 或 D-Bus 密钥存储 crate 时失败，请在 WSL 发行版内安装 Linux 构建依赖，然后重新运行那条 Cargo 包安装命令。它会安装 `codewhale`：

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libdbus-1-dev
cargo install codewhale-cli --locked   # 安装 codewhale
```

预编译的 npm/GitHub 二进制不需要这些构建时包；它们只在 WSL2 从源码编译 Codewhale 时才需要。

### 包装器装好了但找不到 `codewhale`

`npm i -g` 安装到 `$(npm prefix -g)/bin`；请确保该目录在你的 shell `PATH` 上。使用 nvm 时：`nvm use --lts && hash -r`。

### Windows：`rustup-init` 报 `TLS handshake eof` 或 `CRYPT_E_REVOCATION_OFFLINE`

对 `static.rust-lang.org` 的 TLS 握手在 GFW 或某些中国 ISP 后面失败。在运行安装器**之前**设置 rustup 镜像环境变量：

```bash
# git-bash / msys2
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
./rustup-init.exe -y --default-toolchain stable
```

如果 Rust 安装后 Cargo 报 `CRYPT_E_REVOCATION_OFFLINE`，请在 `cargo build` 期间同时设置 `CARGO_HTTP_CHECK_REVOKE=false`。

### Windows：`cargo build` 期间找不到 MSVC 编译器（`cl.exe`）

Visual Studio Build Tools 不会把 `cl.exe` 加入全局 `PATH`。二选一：

1. 从开始菜单打开 **"VS 2022 开发者命令提示符"**，在该窗口中把 `%USERPROFILE%\.cargo\bin` 加入 `PATH`，并从那里运行 `cargo build`；或
2. 手动设置 MSVC 环境变量——PowerShell 片段见[Windows 源码构建](#windows-源码构建)一节。

验证编译器可用：`cl.exe /?` 应打印帮助文本。

### Windows：Cargo 执行构建脚本时报 `拒绝访问 (os error 5)`

第三方杀毒软件（火绒、360、卡巴斯基等）可能阻止 Cargo 执行刚编译的构建脚本二进制（例如 `libsqlite3-sys`、`aws-lc-sys`、`instability`）。该错误与路径无关——移动 `target-dir` 也无济于事。

**症状**：`could not execute process ... build-script-build (never executed)`

**临时方案**（任选其一）：

1. **把项目的 `target/` 目录加入杀毒软件排除列表。**
2. **在 `cargo build` 期间暂时关闭杀毒软件。**
3. **改用 GitHub Release 安装器/压缩包**——发布资源提供预编译二进制，完全跳过 Cargo 构建（[第 6 节](#6-从-github-releases-手动下载)）。
4. **使用 crates.io 的 `cargo install codewhale-cli --locked`**——这会改变二进制路径，某些杀毒软件对不同的路径处理方式不同。

要验证构建脚本二进制本身是否有效（未损坏），在 `target/debug/build/<crate>/build-script-build` 下找到它并手动运行：

```bash
target/debug/build/libsqlite3-sys-*/build-script-build
# 如果它能运行但以 "NotPresent"（没有 C 编译器）panic，说明二进制没问题——
# 是杀毒软件专门在阻止 Cargo 的进程派生路径。
```

### npm 二进制下载超时

如果 `codewhale` 等待几秒后打印 `connect ETIMEDOUT` 或 `EAI_AGAIN`（从 `github.com` 拉取时），说明 npm 包装器安装成功，但预编译二进制下载在你的网络上被屏蔽或不稳定。该下载与 npm registry 包下载是分开的。在 Linux x64 上，包装器先竞争小型 GitHub 和 CNB 校验和清单，不会等到完整 GitHub 二进制超时才使用有效的 CNB 清单。

使用以下路径之一：

1. 设置代理并重试：

   ```bash
   export HTTPS_PROXY=http://your-proxy:port
   codewhale
   ```

2. 在内部镜像 release 资源并设置 `CODEWHALE_RELEASE_BASE_URL`：

   ```bash
   export CODEWHALE_RELEASE_BASE_URL=https://your-mirror.example.com/CodeWhale/
   codewhale
   ```

   目录必须包含 `codewhale-artifacts-sha256.txt` 和来自 GitHub release 的平台二进制。

3. 通过 Cargo 安装，它在本地构建，不下载 GitHub release 资源。参见[第 4 节](#4-通过-cargo-安装任何-tier-1-rust-目标)。

4. 从 [Releases 页面](https://github.com/Hmbown/CodeWhale/releases) 下载匹配的 `codewhale` 和 `codew` 两个二进制，放入 `PATH` 上的目录并赋予可执行权限。参见[第 6 节](#6-从-github-releases-手动下载)。

---

## 10. 验证你的安装

```bash
codewhale --version
codewhale doctor       # 检查 API key、provider、运行时与 PATH 完整性
codewhale doctor --json
```

如果 `doctor` 发现问题，会以非零状态退出并打印结构化的修复提示。需要帮助时，把 JSON 输出粘贴到 GitHub issue 中。
