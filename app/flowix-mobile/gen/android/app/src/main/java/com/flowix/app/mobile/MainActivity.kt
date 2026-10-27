package com.flowix.app.mobile

import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    // 自建云联调：release 包也开放 WebView CDP（adb forward webview_devtools_remote）
    WebView.setWebContentsDebuggingEnabled(true)
    super.onCreate(savedInstanceState)
  }

  /**
   * WebView history 栈底（列表屏）再按系统返回：退后台而非退出 Activity，
   * 保住 30s 云同步轮询。子屏由 WryActivity 的 canGoBack→goBack 处理，不进这里。
   */
  @Suppress("DEPRECATION")
  override fun onBackPressed() {
    moveTaskToBack(true)
  }
}
