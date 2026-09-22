import React from 'react'
import { Icon } from './controls'
import { C } from '../theme'
import type { ConfirmModalOptions } from '../agent/store'

export function ConfirmDialog({
  options,
  onClose,
}: {
  options: ConfirmModalOptions
  onClose: () => void
}) {
  const { title, message, confirmText = '确认删除', cancelText = '取消', onConfirm } = options

  return (
    <div
      testId="confirm-dialog-overlay"
      onClick={onClose}
      style={{
        position: 'absolute',
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: C.scrim,
        pointerEvents: 'auto',
      }}
    >
      <div
        testId="confirm-dialog"
        onClick={(e: any) => e?.stopPropagation?.()}
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: 360,
          maxWidth: '90%',
          backgroundColor: C.raised,
          borderRadius: 12,
          borderWidth: 1,
          borderColor: C.borderStrong,
          boxShadow: {
            offsetX: 0,
            offsetY: 12,
            blurRadius: 36,
            spreadRadius: 0,
            color: C.shadowStrong,
          },
          overflow: 'hidden',
        }}
      >
        {/* 顶部标题栏 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 8,
            paddingLeft: 18,
            paddingRight: 18,
            paddingTop: 16,
            paddingBottom: 8,
            borderTopLeftRadius: 11,
            borderTopRightRadius: 11,
          }}
        >
          <Icon name="alertTriangle" size={16} color={C.accent} />
          <text
            testId="confirm-dialog-title"
            style={{ fontSize: 13.5, fontWeight: 600, color: C.text }}
          >
            {title}
          </text>
        </div>

        {/* 提示正文 */}
        <div
          style={{
            paddingLeft: 18,
            paddingRight: 18,
            paddingTop: 4,
            paddingBottom: 18,
          }}
        >
          <text
            testId="confirm-dialog-message"
            style={{ fontSize: 12.5, lineHeight: 18, color: C.secondary }}
          >
            {message}
          </text>
        </div>

        {/* 底部按钮操作栏 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'flex-end',
            gap: 8,
            paddingLeft: 16,
            paddingRight: 16,
            paddingTop: 10,
            paddingBottom: 12,
            borderTopWidth: 1,
            borderColor: C.border,
            backgroundColor: C.sidebar,
            borderBottomLeftRadius: 11,
            borderBottomRightRadius: 11,
          }}
        >
          <div
            testId="confirm-dialog-cancel"
            role="button"
            aria-label={cancelText}
            onClick={onClose}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              height: 28,
              paddingLeft: 14,
              paddingRight: 14,
              borderRadius: 6,
              cursor: 'pointer',
              backgroundColor: C.chip,
              borderWidth: 1,
              borderColor: C.chipBorder,
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <text style={{ fontSize: 12, color: C.text }}>{cancelText}</text>
          </div>

          <div
            testId="confirm-dialog-confirm"
            role="button"
            aria-label={confirmText}
            onClick={() => {
              onClose()
              onConfirm()
            }}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              height: 28,
              paddingLeft: 14,
              paddingRight: 14,
              borderRadius: 6,
              cursor: 'pointer',
              backgroundColor: C.accent,
              hover: { opacity: 0.9 },
            }}
          >
            <text style={{ fontSize: 12, fontWeight: 500, color: '#ffffff' }}>{confirmText}</text>
          </div>
        </div>
      </div>
    </div>
  )
}
