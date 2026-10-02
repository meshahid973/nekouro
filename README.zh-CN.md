# NekoUro

在本地管理你的编码 agent（Claude Code、Codex、Cursor、Grok、Hermes、Pi、Antigravity），也可以打开多设备同步。

*[English](README.md) | 简体中文*

![NekoUro 驱动一个 Claude Code 会话，侧边栏是实时的分支 diff](apps/landing/public/assets/app-screenshot.jpg)

每台设备各跑一个小引擎，会话就存在这台设备上。装完默认是纯本地模式，不用账号，也不用联网。

## 在本地安装运行（Linux）

```bash
git clone https://github.com/meshahid973/nekouro.git
cd nekouro
cargo run --locked -p nekouro
```

如需可安装的 Linux 包，可运行 `scripts/package-linux.sh`，解压 `target/package` 下生成的压缩包，再运行其中的 `install.sh`。该安装包会写入用户级 `nekouro.desktop` 和图标。

日常命令：

```bash
nekouro status      # 查看本地/同步模式和引擎状态
nekouro update      # 更新到最新版本
nekouro daemon start|stop|restart|status
```

## 可选：多设备同步

只有想打开账号下的同步工作区时才需要登录。登录会换掉引擎下次启动时用的 profile，所以改之前先停掉守护进程：

```bash
nekouro daemon stop
nekouro login
nekouro daemon start
```

之后就可以在一台同步过的设备上起 agent，换另一台设备接着看、接着操作。一台常开的机器，比如 VPS，可以在你合上笔记本之后继续跑这些 agent。

登录不会上传、搬走或导入已有的本地会话。本地会话和它们的附件仍然留在本地 profile 下，切回纯本地模式时会照常出现：

```bash
nekouro daemon stop
nekouro logout
nekouro daemon start
```

如果有引擎正占着数据目录，`nekouro login` 和 `nekouro logout` 会拒绝改动凭据。桌面应用同样遵守这条边界：profile 要等下次重启才切换。

macOS 上用桌面版发行包，或者从源码构建 `nekouro`，再运行 `nekouro daemon install` 装上 launchd 服务。

## 上游与署名

NekoUro 基于 [Zeron](https://github.com/zeronsh/zeron)，并保留 Zeron 的 MIT 许可证声明。上游 Zeron 项目将 [The Context Company](https://www.thecontextcompany.com/) 列为赞助方；相关赞助页面由上游项目维护。

---

想参与开发，或者好奇它怎么跑起来的？可以看 [ARCHITECTURE.md](ARCHITECTURE.md)。上游 Zeron 的资料见[其仓库](https://github.com/zeronsh/zeron)。

采用 [MIT License](LICENSE)。
