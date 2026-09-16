import QtQuick
import QtQuick.Controls
import QtTest

Item {
    id: fixture
    width: 400
    height: 300

    // Panel.qml hosts its content as an initial StackView page and later pushes
    // the discussion chat as a second page. It relies on three StackView facts
    // pinned here: (a) an item pushed with StackView.Immediate becomes
    // currentItem, is resized to fill the view, and hides the page below it;
    // (b) popping back to the initial page with pop(item, StackView.Immediate)
    // restores it as currentItem; (c) a pushed item that was declared as a
    // sibling instance (not created from a Component/delegate) is not
    // destroyed on pop and keeps its child state. (d) a page declared with
    // visible: false stays hidden until pushed, StackView shows it while it is
    // current, and pop hides it again without destroying it.

    StackView {
        id: stack
        anchors.fill: parent
        initialItem: firstPage
    }

    Item {
        id: firstPage
    }

    Item {
        id: secondPage
        TextEdit {
            id: secondPageEdit
            text: "initial"
        }
    }

    Item {
        id: hiddenPage
        visible: false
        Rectangle {
            id: hiddenPageChild
            objectName: "hiddenPageChild"
            width: 50
            height: 20
        }
        TextEdit {
            id: hiddenPageEdit
            text: "initial"
        }
    }

    TestCase {
        name: "PanelStack"
        when: windowShown

        function cleanup() {
            if (stack.currentItem !== firstPage) {
                stack.pop(firstPage, StackView.Immediate)
            }
        }

        function test_push_sizes_hides_and_pop_preserves_state() {
            compare(stack.currentItem, firstPage)
            verify(firstPage.visible, "the initial page starts visible")

            secondPageEdit.text = "edited state"

            stack.push(secondPage, StackView.Immediate)
            compare(stack.currentItem, secondPage, "the pushed instance becomes current")
            compare(secondPage.width, stack.width, "the pushed page fills the view's width")
            compare(secondPage.height, stack.height, "the pushed page fills the view's height")
            verify(!firstPage.visible, "the page below the top is hidden")

            stack.pop(firstPage, StackView.Immediate)
            compare(stack.currentItem, firstPage, "popping restores the initial page as current")
            verify(firstPage.visible)

            verify(secondPage !== null, "the popped instance is not destroyed")
            compare(secondPageEdit.text, "edited state",
                "the popped instance keeps its child state instead of being recreated")
        }

        function test_hidden_declared_page_is_only_visible_while_current() {
            // Initial state: hiddenPage is declared with visible: false
            compare(stack.currentItem, firstPage, "starts with firstPage as current")
            verify(!hiddenPage.visible, "hiddenPage starts hidden")
            verify(!hiddenPageChild.visible, "hidden children are not visible before push")
            verify(firstPage.visible, "firstPage is visible initially")

            // Set draft state and push the hidden page
            hiddenPageEdit.text = "draft"
            stack.push(hiddenPage, StackView.Immediate)

            compare(stack.currentItem, hiddenPage, "hiddenPage becomes current after push")
            verify(hiddenPage.visible, "hiddenPage is visible while current")
            compare(hiddenPage.width, stack.width, "hiddenPage fills the stack width")
            compare(hiddenPage.height, stack.height, "hiddenPage fills the stack height")
            verify(hiddenPageChild.visible, "children are visible when page is visible")
            verify(!firstPage.visible, "firstPage is hidden when hiddenPage is current")

            // Pop back and verify state is preserved
            stack.pop(firstPage, StackView.Immediate)

            compare(stack.currentItem, firstPage, "firstPage is current after pop")
            verify(firstPage.visible, "firstPage is visible after pop")
            verify(!hiddenPage.visible, "hiddenPage is hidden after pop")
            verify(!hiddenPageChild.visible, "children are hidden after pop")
            compare(hiddenPageEdit.text, "draft", "draft state is preserved across push/pop")

            // Push and pop a second time to verify reopening behavior
            stack.push(hiddenPage, StackView.Immediate)

            compare(stack.currentItem, hiddenPage, "hiddenPage is current on second push")
            verify(hiddenPage.visible, "hiddenPage is visible on second push")
            compare(hiddenPage.width, stack.width, "hiddenPage fills stack on second push")
            compare(hiddenPage.height, stack.height, "hiddenPage fills stack on second push")
            verify(hiddenPageChild.visible, "children are visible on second push")
            verify(!firstPage.visible, "firstPage is hidden on second push")
            compare(hiddenPageEdit.text, "draft", "draft state is still preserved")

            stack.pop(firstPage, StackView.Immediate)

            compare(stack.currentItem, firstPage, "firstPage is current after second pop")
            verify(firstPage.visible, "firstPage is visible after second pop")
            verify(!hiddenPage.visible, "hiddenPage is hidden after second pop")
            verify(!hiddenPageChild.visible, "children are hidden after second pop")
        }
    }
}
