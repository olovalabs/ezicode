use anyhow::Result;
use gpui::{Context, EntityInputHandler, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, InlineCompletionContext,
    InlineCompletionItem, InlineCompletionResponse, InlineCompletionTriggerKind,
    request::Completion,
};
use ropey::Rope;
use std::{cell::RefCell, ops::Range, rc::Rc, time::Duration};

use crate::input::{
    InputState,
    popovers::{CompletionMenu, ContextMenu},
};

const DEFAULT_INLINE_COMPLETION_DEBOUNCE: Duration = Duration::from_millis(300);

pub trait CompletionProvider {

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>>;

    fn inline_completion(
        &self,
        _rope: &Rope,
        _offset: usize,
        _trigger: InlineCompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<Result<InlineCompletionResponse>> {
        Task::ready(Ok(InlineCompletionResponse::Array(vec![])))
    }

    #[inline]
    fn inline_completion_debounce(&self) -> Duration {
        DEFAULT_INLINE_COMPLETION_DEBOUNCE
    }

    fn resolve_completions(
        &self,
        _completion_indices: Vec<usize>,
        _completions: Rc<RefCell<Box<[Completion]>>>,
        _: &mut Context<InputState>,
    ) -> Task<Result<bool>> {
        Task::ready(Ok(false))
    }

    fn is_completion_trigger(
        &self,
        offset: usize,
        new_text: &str,
        cx: &mut Context<InputState>,
    ) -> bool;
}

pub(crate) struct InlineCompletion {

    pub(crate) item: Option<InlineCompletionItem>,

    pub(crate) task: Task<Result<InlineCompletionResponse>>,
}

impl Default for InlineCompletion {
    fn default() -> Self {
        Self {
            item: None,
            task: Task::ready(Ok(InlineCompletionResponse::Array(vec![]))),
        }
    }
}

impl InputState {
    pub(crate) fn handle_completion_trigger(
        &mut self,
        range: &Range<usize>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion_inserting {
            return;
        }

        let Some(provider) = self.lsp.completion_provider.clone() else {
            return;
        };

        self.schedule_inline_completion(window, cx);

        let start = range.end;
        let new_offset = self.cursor();

        if !provider.is_completion_trigger(start, new_text, cx) {
            return;
        }

        let menu = match self.context_menu.as_ref() {
            Some(ContextMenu::Completion(menu)) => Some(menu),
            _ => None,
        };

        let menu = match menu {
            Some(menu) => menu.clone(),
            None => {
                let menu = CompletionMenu::new(cx.entity(), window, cx);
                self.context_menu = Some(ContextMenu::Completion(menu.clone()));
                menu
            }
        };

        let start_offset = menu.read(cx).trigger_start_offset.unwrap_or(start);
        if new_offset < start_offset {
            return;
        }

        let query = self
            .text_for_range(
                self.range_to_utf16(&(start_offset..new_offset)),
                &mut None,
                window,
                cx,
            )
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        _ = menu.update(cx, |menu, _| {
            menu.update_query(start_offset, query.clone());
        });

        let completion_context = CompletionContext {
            trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
            trigger_character: Some(query),
        };

        let provider_responses =
            provider.completions(&self.text, new_offset, completion_context, window, cx);
        self._context_menu_task = cx.spawn_in(window, async move |editor, cx| {
            let mut completions: Vec<CompletionItem> = vec![];
            if let Some(provider_responses) = provider_responses.await.ok() {
                match provider_responses {
                    CompletionResponse::Array(items) => completions.extend(items),
                    CompletionResponse::List(list) => completions.extend(list.items),
                }
            }

            if completions.is_empty() {
                _ = menu.update(cx, |menu, cx| {
                    menu.hide(cx);
                    cx.notify();
                });

                return Ok(());
            }

            editor
                .update_in(cx, |editor, window, cx| {
                    if !editor.focus_handle.is_focused(window) {
                        return;
                    }

                    _ = menu.update(cx, |menu, cx| {
                        menu.show(new_offset, completions, window, cx);
                    });

                    cx.notify();
                })
                .ok();

            Ok(())
        });
    }

    pub(crate) fn schedule_inline_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {

        self.clear_inline_completion(cx);

        let Some(provider) = self.lsp.completion_provider.clone() else {
            return;
        };

        let offset = self.cursor();
        let text = self.text.clone();
        let debounce = provider.inline_completion_debounce();

        self.inline_completion.task = cx.spawn_in(window, async move |editor, cx| {

            smol::Timer::after(debounce).await;

            let task = editor.update_in(cx, |editor, window, cx| {

                if editor.cursor() != offset {
                    return None;
                }

                if editor.is_context_menu_open(cx) {
                    return None;
                }

                let trigger = InlineCompletionContext {
                    trigger_kind: InlineCompletionTriggerKind::Automatic,
                    selected_completion_info: None,
                };

                Some(provider.inline_completion(&text, offset, trigger, window, cx))
            })?;

            let Some(task) = task else {
                return Ok(InlineCompletionResponse::Array(vec![]));
            };

            let response = task.await?;

            editor.update_in(cx, |editor, _window, cx| {

                if editor.cursor() != offset {
                    return;
                }

                if editor.is_context_menu_open(cx) {
                    return;
                }

                if let Some(item) = match response.clone() {
                    InlineCompletionResponse::Array(items) => items.into_iter().next(),
                    InlineCompletionResponse::List(comp_list) => comp_list.items.into_iter().next(),
                } {
                    editor.inline_completion.item = Some(item);
                    cx.notify();
                }
            })?;

            Ok(response)
        });
    }

    #[inline]
    pub(crate) fn has_inline_completion(&self) -> bool {
        self.inline_completion.item.is_some()
    }

    pub(crate) fn clear_inline_completion(&mut self, cx: &mut Context<Self>) {
        self.inline_completion = InlineCompletion::default();
        cx.notify();
    }

    pub(crate) fn accept_inline_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(completion_item) = self.inline_completion.item.take() else {
            return false;
        };

        let cursor = self.cursor();
        let range_utf16 = self.range_to_utf16(&(cursor..cursor));
        let completion_text = completion_item.insert_text;
        self.replace_text_in_range_silent(Some(range_utf16), &completion_text, window, cx);
        true
    }
}
