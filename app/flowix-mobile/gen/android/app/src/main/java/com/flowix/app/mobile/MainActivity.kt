package com.flowix.app.mobile

import android.content.Context
import android.os.Bundle
import android.view.MotionEvent
import android.view.View
import android.view.inputmethod.InputMethodManager
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    // 自建云联调：release 包也开放 WebView CDP（adb forward webview_devtools_remote）
    WebView.setWebContentsDebuggingEnabled(true)
    super.onCreate(savedInstanceState)
    // 键盘修复①：WebView 触摸焦点模式兜底（部分设备上 wry 未设导致 IME 不跟焦点）
    window.decorView.post {
      findWebView()?.let { it.isFocusableInTouchMode = true; it.requestFocus() }
    }
  }

  /**
   * WebView history 栈底（列表屏）再按系统返回：退后台而非退出 Activity，
   * 保住 30s 云同步轮询。子屏由 WryActivity 的 canGoBack→goBack 处理，不进这里。
   */
  @Suppress("DEPRECATION")
  override fun onBackPressed() {
    moveTaskToBack(true)
  }

  /**
   * 键盘修复②：contenteditable 获得光标但 InputMethodManager 不弹软键盘的场景
   * （Android WebView 已知行为）。ACTION_UP 后延时评估页面 activeElement 是否
   * 可编辑——是才 showSoftInput（EXPLICIT），点非编辑区/工具栏不误弹。
   *
   * 竞态说明：tap 的合成 click → focus 切换在 Chromium 内部是异步的，
   * ACTION_UP 同步评估会拿到旧 activeElement（人手点按尤其易踩中），
   * 故 postDelayed 让焦点先落定再判定。
   */
  override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
    if (ev.actionMasked == MotionEvent.ACTION_UP) {
      val wv = currentFocus as? WebView ?: findWebView()
      android.util.Log.i("FlowixKB", "tap-up: webview=${wv != null} focus=${currentFocus?.javaClass?.simpleName}")
      wv?.let { webView ->
        webView.postDelayed({
          webView.evaluateJavascript(
            "(function(){var e=document.activeElement;return !!(e&&(e.isContentEditable||/^(INPUT|TEXTAREA)$/.test(e.tagName)))})()"
          ) { result ->
            android.util.Log.i("FlowixKB", "editable=$result shown_pre=${(getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager).isAcceptingText}")
            if (result == "true") {
              val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
              val ok = imm.showSoftInput(webView, InputMethodManager.SHOW_FORCED)
              android.util.Log.i("FlowixKB", "showSoftInput->$ok")
            }
          }
        }, 180L)
      }
    }
    return super.dispatchTouchEvent(ev)
  }

  private fun findWebView(): WebView? = findWebView(window.decorView)

  private fun findWebView(view: View): WebView? {
    if (view is WebView) return view
    if (view is android.view.ViewGroup) {
      for (i in 0 until view.childCount) {
        findWebView(view.getChildAt(i))?.let { return it }
      }
    }
    return null
  }
}
