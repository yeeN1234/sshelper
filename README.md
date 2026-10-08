<img src="assets/icon/png/sshelper-128.png" width="96" alt="sshelper 圖示">

# sshelper

把本機的 SSH 公鑰部署到遠端主機，並自動寫好 `~/.ssh/config`，之後只要輸入 `ssh <別名>` 就能免密碼登入。

- **跨平台**：本機支援 Windows / Linux / macOS；目標主機支援 Linux 與 Windows（OpenSSH Server）
- **兩種介面**：圖形介面 `sshelper-gui`、命令列 `sshelper`（互動精靈或全參數無人值守）
- **不依賴系統的 ssh / scp**：以純 Rust 的 [russh](https://github.com/Eugeny/russh) 連線，密碼只需輸入一次

## 建置

需要 Rust stable（以 1.99 開發與測試）。

```sh
cargo build --release
# 產出：target/release/sshelper(.exe)、target/release/sshelper-gui(.exe)
```

Linux 編譯 GUI 需要的套件（Debian / Ubuntu）：

```sh
sudo apt-get install -y libxkbcommon-dev libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

## 使用方式

### 圖形介面

執行 `sshelper-gui`（或 `sshelper gui`），依序：

1. 選擇 `~/.ssh` 中的公鑰（自動對應同檔名的私鑰）
2. 填入主機、連接埠、帳號（預設 `ubuntu`，也可直接輸入 `user@host`）、作業系統（可自動偵測）、密碼
3. 輸入連線別名；別名已存在時會提示是否覆蓋
4. 按「開始部署」或在任一欄位按 Enter。第一次連線會顯示主機指紋請你確認，完成後顯示 `ssh <別名>` 與複製按鈕

畫面配置：

- 單一精簡表單，預設視窗大小即可看到全部欄位，不需捲動或拉大視窗
- 按下部署後，同一區域切換為步驟進度與結果；完成後按「部署另一台」回到表單，失敗時可「返回修改」或直接「重試」
- 底列固定顯示主要按鈕，旁邊列出還缺哪些欄位
- 右上角可切換淺色 / 深色主題（淡入淡出）

### 命令列

```sh
sshelper                                   # 互動式精靈，逐步詢問
sshelper -H 192.168.1.10 -a jetson         # 只詢問缺少的項目（金鑰、密碼）
sshelper keys                              # 列出 ~/.ssh 中的金鑰
```

無人值守（CI、批次部署）：

```sh
echo "$PASSWORD" | sshelper -k id_ed25519 -H ubuntu@10.0.0.5 -a dev \
    --accept-new-host-key --password-stdin
```

| 選項 | 說明 |
| --- | --- |
| `-k, --key` | 金鑰名稱（`id_ed25519`）或檔案路徑 |
| `-H, --host` | 目標主機，可寫成 `user@host` |
| `-u, --user` / `-p, --port` | 帳號（預設 `ubuntu`）/ 連接埠（預設 22） |
| `--os auto\|linux\|windows` | 目標作業系統（預設自動偵測） |
| `-a, --alias` | `~/.ssh/config` 的 `Host` 名稱 |
| `--replace` | 別名已存在時覆蓋該區塊 |
| `--accept-new-host-key` | 自動信任未記錄的主機金鑰（金鑰**變更**時仍會拒絕） |
| `--password-stdin` | 從標準輸入讀密碼；也可用環境變數 `SSHELPER_PASSWORD` |
| `--no-verify` / `-y` | 不做登入測試 / 略過確認 |
| `--ssh-dir` | 改用其他 `.ssh` 目錄（環境變數 `SSHELPER_SSH_DIR`，GUI 也適用） |

結束代碼：`0` 成功、`1` 失敗、`3` 已部署但免密碼登入測試未通過、`130` 已取消。

## 部署流程

1. **連線**：檢查主機金鑰與 `~/.ssh/known_hosts`。未記錄時顯示指紋請使用者確認後寫入（OpenSSH 格式，非 22 port 為 `[host]:port`）；金鑰不符時直接中止
2. **登入**：密碼登入，伺服器只提供 keyboard-interactive（PAM）時會自動改用
3. **修正本機私鑰權限**：Windows 以 `icacls` 關閉繼承、只授權目前使用者 (R,W)；Unix 為 `chmod 600`
4. **確認遠端作業系統**：依 SFTP 回報的家目錄判斷（`/C:/Users/...` 為 Windows）；與選擇不符時改用偵測結果
5. **上傳**：以 SFTP 把 `.pub` **原封不動**上傳為 `~/.sshelper-<亂數>.pub`，連同安裝腳本
6. **遠端安裝**
   - Linux：`tr -d '\r'` 去除 CR、去除 BOM、略過已存在的金鑰、補上缺少的結尾換行後追加；`~/.ssh` 設 700、`authorized_keys` 設 600；SELinux 會執行 `restorecon`；家目錄可被他人寫入時提出警告
   - Windows：寫入 `%USERPROFILE%\.ssh\authorized_keys`；管理員帳號且 `sshd_config` 有 `Match Group administrators` 時，另寫入 `%ProgramData%\ssh\administrators_authorized_keys`。一律存成 UTF-8（無 BOM）、LF，並重設 ACL
   - 兩種腳本最後都會刪除暫存檔
7. **寫入 `~/.ssh/config`**：`Host`、`HostName`、`User`、`Port`（非 22 時）、`IdentityFile`、`IdentitiesOnly yes`
   - 有 `Host *` 時插在它前面，避免被萬用設定蓋掉
   - 同名區塊原地覆蓋，修改前先備份成 `config.bak-<時間>`
   - BOM、UTF-16 編碼的 config 會轉回 UTF-8
8. **驗證**：以系統 `ssh -o BatchMode=yes <別名> exit` 實測；沒有安裝 ssh 時改用內建用戶端以金鑰登入

### 為什麼不用 PowerShell 管線傳公鑰

PowerShell 會把字串轉成 UTF-16 或加上 UTF-8 BOM，換行也會變成 CRLF，寫進 `authorized_keys` 後 sshd 會拒絕這把金鑰。sshelper 以位元組原樣上傳檔案，再由遠端清除 CR 與 BOM。

`--password-stdin` 同樣會移除 PowerShell 管線加在開頭的 BOM。

## 專案結構

```
crates/
  sshelper-core/     核心函式庫（無 UI）
    src/keys.rs        金鑰探索、公鑰解析、私鑰權限
    src/config.rs      ~/.ssh/config 解析與合併
    src/remote.rs      russh 連線、主機金鑰、認證、SFTP、遠端執行
    src/deploy.rs      部署步驟與進度事件
    src/verify.rs      部署後登入測試
    src/validate.rs    使用者輸入驗證（CLI / GUI 共用）
    assets/remote/     遠端安裝腳本 install-key.sh / install-key.ps1（編譯時內嵌）
  sshelper-cli/      sshelper：clap + inquire
  sshelper-gui/      sshelper-gui：eframe / egui
```

## 測試

```sh
cargo test --workspace
```

- **core**：config 合併（`Host *` 插入、原地覆蓋、BOM/UTF-16）、公鑰解析、輸入驗證、known_hosts 寫入
- **遠端腳本**：`tests/remote_scripts.rs` 以假的家目錄實際執行 `install-key.sh`（有 `sh` 時）與 `install-key.ps1`（Windows）
- **GUI**：以 [egui_kittest](https://crates.io/crates/egui_kittest) 驅動實際介面（填表、別名衝突、`user@host` 拆分、無金鑰提示）

端對端測試（需要 Docker）：

```sh
docker run -d --rm --name sshelper-e2e -p 127.0.0.1:2222:2222 \
  -e PUID=1000 -e PGID=1000 -e PASSWORD_ACCESS=true \
  -e USER_NAME=tester -e USER_PASSWORD=sshelper-test \
  lscr.io/linuxserver/openssh-server:latest

# GUI 全流程
SSHELPER_E2E_TARGET=tester@127.0.0.1 SSHELPER_E2E_PORT=2222 SSHELPER_E2E_PASSWORD=sshelper-test \
  cargo test -p sshelper-gui -- --ignored

# CLI 全流程（使用暫存的 .ssh 目錄，不動到自己的設定）
dir=$(mktemp -d) && ssh-keygen -q -t ed25519 -N "" -f "$dir/id_e2e"
echo sshelper-test | cargo run -p sshelper-cli -- --ssh-dir "$dir" -k id_e2e \
  -H tester@127.0.0.1 -p 2222 -a e2e --accept-new-host-key --password-stdin

docker stop sshelper-e2e
```

## 常見問題

- **主機金鑰不符**：主機重灌過金鑰時，確認無誤後執行 `ssh-keygen -R <host>`（非 22 port 為 `ssh-keygen -R "[host]:port"`），再重新部署
- **伺服器不接受密碼登入**：sshelper 需要先以密碼登入一次才能放入金鑰；請暫時開啟 `PasswordAuthentication`，或改用主控台登入
- **遠端未提供 SFTP**：`sshd_config` 需要 `Subsystem sftp ...`（多數發行版預設已開啟）
- **免密碼測試未通過**
  - 私鑰設有 passphrase（請搭配 ssh-agent）
  - Linux 家目錄權限過寬（`chmod go-w ~`）
  - 用 `ssh -v <別名>` 查看詳細過程
- **GUI 中文顯示成方塊**：設定環境變數 `SSHELPER_FONT` 指向任一中文字型檔（.ttf / .ttc）

## 授權

[MIT](LICENSE)。應用程式圖示（`assets/icon/`：SVG、PNG、Windows `.ico`、macOS `.icns`）同為 MIT，見 [assets/icon/LICENSE](assets/icon/LICENSE)；Windows 版編譯時會由 `build.rs` 把 `.ico` 嵌入執行檔（需要 Windows SDK 的 `rc.exe`，安裝 Visual Studio Build Tools 時已附）。
