# Moo — Multiple Operators Orchestrated

<p align="center">
  <img src="assets/moo-logo.png" alt="Moo logo" width="180" />
</p>

Moo 是面向编程智能体的终端工作空间，支持持久会话、智能体团队、任务交接和网页客户端。你可以同时运行多个项目的智能体，查看它们的状态，并协调工作。

Moo 目前仍处于实验阶段。Linux 是主要开发平台；其他平台的验证情况请参阅[安装指南](https://github.com/allisonhere/moo/wiki/Docs-install)。

## 功能

- 后台服务器在客户端断开后保持窗格进程运行。
- 侧栏显示智能体状态，支持命名、分组和任务交接。
- 团队工作流提供计划审核、角色分工、审批和用量限制。
- Ovis 网页客户端支持浏览器和手机访问。
- `moo` 与 `moo-dev` 使用各自的配置、状态和套接字目录。

## 安装

Linux 预编译二进制文件会经过校验，默认安装到 `~/.local/bin`：

```sh
curl -fsSL https://raw.githubusercontent.com/allisonhere/moo/master/scripts/install.sh | sh
```

从源码构建需要 Rust 和 Zig 0.16.0：

```sh
git clone https://github.com/allisonhere/moo
cd moo
./scripts/install-moo.sh
```

运行 `moo` 启动。按 `ctrl+b q` 分离，再运行 `moo` 重新连接。更新时重新运行安装命令；`moo update` 不会下载二进制文件。

## 文档

[完整文档](https://github.com/allisonhere/moo/wiki/Documentation)包括安装、配置、命令、远程连接、插件、[团队](https://github.com/allisonhere/moo/wiki/Docs-teams)和[网页客户端](https://github.com/allisonhere/moo/wiki/Web-UI-with-Ovis)。中文文档源文件位于 `docs/next/website/src/content/docs/zh-cn/`。

## 许可证与致谢

Moo 基于 [Herdr](https://github.com/herdrdev/herdr)，采用 [Apache License 2.0](LICENSE)。有关来源和文档修改，请参阅[致谢](https://github.com/allisonhere/moo/wiki/Docs-credits)。
