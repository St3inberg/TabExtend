package io.github.st3inberg.tabextend

import android.os.SystemClock
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import kotlin.math.roundToInt

/**
 * Turns the tablet's touch, pen, mouse and keyboard events into protocol
 * messages, with positions as fractions of the view.
 *
 * - Fingers become multi-touch, so GNOME's own touch gestures work.
 * - The pen becomes a precise pointer: hover moves it, contact clicks and
 *   drags, the barrel button right-clicks. (The remote-desktop portal has
 *   no pen device yet, so pressure and tilt cannot be sent.)
 * - Palm rejection: fingers are ignored while the pen is touching or
 *   hovering, and fingers already down are cancelled when it arrives.
 * - A mouse or trackpad becomes pointer, buttons and wheel.
 */
class InputTranslator(private val size: () -> Pair<Int, Int>) {
    private val slots = SlotTable(MAX_SLOTS)
    private var lastPenMs = Long.MIN_VALUE / 2
    private var penDown = false
    private var penButtons = 0
    private var mouseButtons = 0

    fun onTouch(event: MotionEvent): List<Message> {
        val out = mutableListOf<Message>()
        mouseButtonChanges(event, out)
        val index = event.actionIndex
        for (i in 0 until event.pointerCount) {
            when (event.getToolType(i)) {
                MotionEvent.TOOL_TYPE_STYLUS, MotionEvent.TOOL_TYPE_ERASER -> pen(event, i, i == index, out)
                MotionEvent.TOOL_TYPE_MOUSE -> out += pointer(event.getX(i), event.getY(i))
                else -> finger(event, i, i == index, out)
            }
        }
        return out
    }

    /** Hover (pen or mouse) and wheel: events with no contact. */
    fun onGenericMotion(event: MotionEvent): List<Message> {
        val out = mutableListOf<Message>()
        val isPen = event.getToolType(0).let {
            it == MotionEvent.TOOL_TYPE_STYLUS || it == MotionEvent.TOOL_TYPE_ERASER
        }
        when (event.actionMasked) {
            MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_MOVE -> {
                if (isPen) penNearby(out)
                out += pointer(event.x, event.y)
                if (isPen) penButtonChanges(event, out)
            }
            MotionEvent.ACTION_HOVER_EXIT -> if (isPen) lastPenMs = SystemClock.uptimeMillis()
            MotionEvent.ACTION_SCROLL -> {
                out += pointer(event.x, event.y)
                // Android: vertical +1 is up. Protocol: positive is down.
                val vertical = -event.getAxisValue(MotionEvent.AXIS_VSCROLL)
                val horizontal = event.getAxisValue(MotionEvent.AXIS_HSCROLL)
                val steps = { v: Float -> if (v == 0f) 0 else v.roundToInt().takeIf { it != 0 } ?: if (v > 0) 1 else -1 }
                if (vertical != 0f || horizontal != 0f) out += Message.Scroll(steps(horizontal), steps(vertical))
            }
        }
        mouseButtonChanges(event, out)
        return out
    }

    /** A key from a physical keyboard, or null if it should stay on the tablet. */
    fun onKey(event: KeyEvent): Message? {
        val device = event.device ?: return null
        val keyboard = event.source and InputDevice.SOURCE_KEYBOARD == InputDevice.SOURCE_KEYBOARD
        if (!device.isExternal || !keyboard || event.repeatCount > 0) return null
        if (event.scanCode !in 1..KEY_MAX) return null
        return when (event.action) {
            KeyEvent.ACTION_DOWN -> Message.Key(event.scanCode, true)
            KeyEvent.ACTION_UP -> Message.Key(event.scanCode, false)
            else -> null
        }
    }

    /** Lift everything, e.g. when the connection or the window goes away. */
    fun reset() {
        slots.clear()
        penDown = false
        penButtons = 0
        mouseButtons = 0
    }

    private fun pointer(x: Float, y: Float): Message.PointerMove {
        val (w, h) = size()
        return Message.PointerMove(x / w, y / h)
    }

    private fun touch(slot: Int, phase: TouchPhase, x: Float, y: Float): Message.Touch {
        val (w, h) = size()
        return Message.Touch(slot, phase, x / w, y / h)
    }

    private fun penNearby(out: MutableList<Message>) {
        lastPenMs = SystemClock.uptimeMillis()
        for (slot in slots.releaseAll()) out += Message.Touch(slot, TouchPhase.CANCEL, 0f, 0f)
    }

    private fun pen(event: MotionEvent, i: Int, isActionPointer: Boolean, out: MutableList<Message>) {
        penNearby(out)
        out += pointer(event.getX(i), event.getY(i))
        val action = event.actionMasked
        val down = action == MotionEvent.ACTION_DOWN || action == MotionEvent.ACTION_POINTER_DOWN
        val up = action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_POINTER_UP ||
            action == MotionEvent.ACTION_CANCEL
        if (isActionPointer && down && !penDown) {
            penDown = true
            out += Message.Button(MouseButton.LEFT, true)
        } else if ((isActionPointer && up || action == MotionEvent.ACTION_CANCEL) && penDown) {
            penDown = false
            out += Message.Button(MouseButton.LEFT, false)
        }
        penButtonChanges(event, out)
    }

    /** The pen's barrel button acts as the right mouse button. */
    private fun penButtonChanges(event: MotionEvent, out: MutableList<Message>) {
        val now = event.buttonState and (MotionEvent.BUTTON_STYLUS_PRIMARY or MotionEvent.BUTTON_SECONDARY)
        val pressed = now != 0
        if (pressed != (penButtons != 0)) out += Message.Button(MouseButton.RIGHT, pressed)
        penButtons = now
    }

    private fun finger(event: MotionEvent, i: Int, isActionPointer: Boolean, out: MutableList<Message>) {
        val id = event.getPointerId(i)
        if (SystemClock.uptimeMillis() - lastPenMs < PALM_REJECTION_MS) {
            slots.release(id)?.let { out += Message.Touch(it, TouchPhase.CANCEL, 0f, 0f) }
            return
        }
        val x = event.getX(i)
        val y = event.getY(i)
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> if (isActionPointer) {
                slots.acquire(id)?.let { out += touch(it, TouchPhase.DOWN, x, y) }
            } else {
                slots.slotOf(id)?.let { out += touch(it, TouchPhase.MOVE, x, y) }
            }
            MotionEvent.ACTION_MOVE -> slots.slotOf(id)?.let { out += touch(it, TouchPhase.MOVE, x, y) }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> if (isActionPointer) {
                slots.release(id)?.let { out += touch(it, TouchPhase.UP, x, y) }
            }
            MotionEvent.ACTION_CANCEL -> slots.release(id)?.let { out += Message.Touch(it, TouchPhase.CANCEL, 0f, 0f) }
        }
    }

    private fun mouseButtonChanges(event: MotionEvent, out: MutableList<Message>) {
        if (!event.isFromSource(InputDevice.SOURCE_MOUSE)) return
        val now = event.buttonState and MOUSE_BUTTONS.keys.fold(0) { acc, b -> acc or b }
        if (now == mouseButtons) return
        out += pointer(event.x, event.y)
        for ((bit, button) in MOUSE_BUTTONS) {
            val was = mouseButtons and bit != 0
            val isNow = now and bit != 0
            if (was != isNow) out += Message.Button(button, isNow)
        }
        mouseButtons = now
    }

    private companion object {
        const val MAX_SLOTS = 10
        const val KEY_MAX = 0x2ff
        /** How long after the pen was last seen fingers stay ignored. */
        const val PALM_REJECTION_MS = 400L
        val MOUSE_BUTTONS = linkedMapOf(
            MotionEvent.BUTTON_PRIMARY to MouseButton.LEFT,
            MotionEvent.BUTTON_SECONDARY to MouseButton.RIGHT,
            MotionEvent.BUTTON_TERTIARY to MouseButton.MIDDLE,
            MotionEvent.BUTTON_BACK to MouseButton.BACK,
            MotionEvent.BUTTON_FORWARD to MouseButton.FORWARD,
        )
    }
}

/**
 * Gives each finger (Android pointer id) a small slot number for as long as
 * it is down. Pointer ids can be large and are reused; slots stay below
 * the host's limit.
 */
class SlotTable(private val capacity: Int) {
    private val byPointer = linkedMapOf<Int, Int>()

    fun acquire(pointerId: Int): Int? {
        byPointer[pointerId]?.let { return it }
        val free = (0 until capacity).firstOrNull { it !in byPointer.values } ?: return null
        byPointer[pointerId] = free
        return free
    }

    fun slotOf(pointerId: Int): Int? = byPointer[pointerId]

    fun release(pointerId: Int): Int? = byPointer.remove(pointerId)

    fun releaseAll(): List<Int> = byPointer.values.toList().also { byPointer.clear() }

    fun clear() = byPointer.clear()
}
