!macro NSIS_HOOK_PREINSTALL
  ; ===== 覆盖安装 =====
  ; 目标:
  ;   1) 若旧实例在运行(含其拉起的 node/git 等子进程),先整棵进程树关闭,
  ;      确保安装目录内无文件被占用;
  ;   2) 等待 OS 释放被杀进程的文件句柄;
  ;   3) 整目录删除旧安装目录,实现"完全覆盖重装"。
  ; 注意:一律不用 /REBOOTOK —— 宁可安装失败/留残余,也不向系统请求重启。
  nsExec::ExecToLog 'taskkill /F /T /IM "pi-desktop.exe"'
  Sleep 1500
  RMDir /r "$INSTDIR"
  IfErrors 0 +2
    DetailPrint "WARN: 旧安装目录有文件未能删除(可能仍被占用),继续安装…"
!macroend

!macro NSIS_HOOK_POSTINSTALL

!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; ===== 卸载前关闭旧实例树 =====
  ; 关闭主程序及其子进程,等待句柄释放,确保安装目录内文件可被删除。
  ; (与安装钩子一致:不用 /REBOOTOK,避免卸载触发系统重启提示)
  nsExec::ExecToLog 'taskkill /F /T /IM "pi-desktop.exe"'
  Sleep 1500
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; ===== 卸载后删除整个安装目录 =====
  ; 文件/注册表/快捷方式移除后,把安装目录(含 node/runtime/logs/config 等残留)
  ; 一并清掉,避免卸载后遗留整个目录。卸载器本体运行于临时副本,可安全删除目录。
  RMDir /r "$INSTDIR"
!macroend
