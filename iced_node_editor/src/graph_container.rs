use iced::{
    advanced::{
        layout, renderer,
        widget::{self, Operation},
        Clipboard, Layout, Shell, Widget,
    },
    event, mouse, Background, Border, Color, Element, Event, Length, Point, Rectangle, Size,
    Vector,
};
use std::collections::VecDeque;
use std::sync::Mutex;

use crate::connection::LogicalEndpoint;
use crate::node_element::SocketLayoutState;
use crate::{
    matrix::Matrix,
    styles::graph_container::{Appearance, StyleSheet},
    Endpoint, GraphNodeElement, Link, SocketRole,
};

pub struct GraphContainer<'a, Message, Theme, Renderer>
where
    Theme: StyleSheet,
    Renderer: renderer::Renderer,
{
    width: Length,
    height: Length,
    max_width: f32,
    max_height: f32,
    style: Theme::Style,
    content: Vec<GraphNodeElement<'a, Message, Theme, Renderer>>,
    matrix: Matrix,
    on_translate: Option<Box<dyn Fn((f32, f32)) -> Message + 'a>>,
    on_scale: Option<Box<dyn Fn(f32, f32, f32) -> Message + 'a>>,
    on_connect: Option<Box<dyn Fn(Link) -> Message + 'a>>,
    on_disconnect: Option<Box<dyn Fn(LogicalEndpoint, Point) -> Message + 'a>>,
    on_dangling: Option<Box<dyn Fn(Option<(LogicalEndpoint, Link)>) -> Message + 'a>>,
    dangling_source: Option<LogicalEndpoint>,

    phantom_message: std::marker::PhantomData<Message>,
    socket_state: Mutex<SocketLayoutState>,
}

struct GraphContainerState {
    drag_start_position: Option<Point>,
}

impl<'a, Message, Theme, Renderer> GraphContainer<'a, Message, Theme, Renderer>
where
    Theme: StyleSheet,
    Renderer: renderer::Renderer,
{
    pub fn new(content: Vec<GraphNodeElement<'a, Message, Theme, Renderer>>) -> Self {
        GraphContainer {
            on_translate: None,
            on_scale: None,
            on_connect: None,
            on_disconnect: None,
            on_dangling: None,
            matrix: Matrix::identity(),
            width: Length::Shrink,
            height: Length::Shrink,
            max_width: f32::MAX,
            max_height: f32::MAX,
            style: Default::default(),
            content,
            dangling_source: None,

            phantom_message: std::marker::PhantomData,
            socket_state: Mutex::new(SocketLayoutState {
                inputs: vec![],
                outputs: vec![],
                done: false,
            }),
        }
    }

    pub fn on_translate<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn((f32, f32)) -> Message,
    {
        self.on_translate = Some(Box::new(f));
        self
    }

    pub fn on_scale<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn(f32, f32, f32) -> Message,
    {
        self.on_scale = Some(Box::new(f));
        self
    }

    pub fn on_connect<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn(Link) -> Message,
    {
        self.on_connect = Some(Box::new(f));
        self
    }

    pub fn on_disconnect<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn(LogicalEndpoint, Point) -> Message,
    {
        self.on_disconnect = Some(Box::new(f));
        self
    }

    pub fn on_dangling<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn(Option<(LogicalEndpoint, Link)>) -> Message,
    {
        self.on_dangling = Some(Box::new(f));
        self
    }

    pub fn matrix(mut self, m: Matrix) -> Self {
        self.matrix = m;
        self
    }

    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = max_width;
        self
    }

    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = max_height;
        self
    }

    pub fn style(mut self, style: impl Into<Theme::Style>) -> Self {
        self.style = style.into();
        self
    }

    pub fn dangling_source(mut self, dangling_source: Option<LogicalEndpoint>) -> Self {
        self.dangling_source = dangling_source;
        self
    }

    fn try_emit_dangling(
        &self,
        shell: &mut Shell<'_, Message>,
        cursor_position: Point,
        source: LogicalEndpoint,
    ) {
        if let Some(f) = &self.on_dangling {
            shell.publish(f(Some((
                source,
                Link::from_unordered(
                    Endpoint::Socket(source),
                    Endpoint::Absolute(cursor_position),
                ),
            ))));
        }
    }
}

pub fn graph_container<Message, Theme, Renderer>(
    content: Vec<GraphNodeElement<Message, Theme, Renderer>>,
) -> GraphContainer<Message, Theme, Renderer>
where
    Theme: StyleSheet,
    Renderer: renderer::Renderer,
{
    GraphContainer::new(content)
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for GraphContainer<'a, Message, Theme, Renderer>
where
    Theme: StyleSheet,
    Renderer: renderer::Renderer,
{
    fn children(&self) -> Vec<widget::Tree> {
        let mut children = Vec::new();

        for node in &self.content {
            children.push(widget::Tree::new(node));
        }

        children
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(self.content.as_slice())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<widget::tree::State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(GraphContainerState {
            drag_start_position: None,
        })
    }

    fn layout(
        &self,
        tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let limits = limits
            .loose()
            .max_width(self.max_width)
            .max_height(self.max_height)
            .width(self.width)
            .height(self.height);

        let mut content = Vec::new();

        let scale = self.matrix.get_scale();
        let offset = self.matrix.get_translation();

        let mut socket_layout_state = self
            .socket_state
            .lock()
            .expect("should be able to lock socket state mutex in layout()");
        socket_layout_state.clear();

        for (node_index, node) in self.content.iter().enumerate() {
            let mut node = node.as_scalable_widget().layout(
                &mut tree.children[node_index],
                _renderer,
                &limits,
                scale,
                &mut socket_layout_state,
            );
            node = node.translate(Vector::new(offset.0, offset.1));

            content.push(node);
        }

        let size = limits.resolve(self.width, self.height, Size::ZERO);

        layout::Node::with_children(size, content)
    }

    fn operate(
        &self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation<Message>,
    ) {
        operation.container(None, layout.bounds(), &mut |operation| {
            self.content
                .iter()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, state), layout)| {
                    child
                        .as_widget()
                        .operate(state, layout, renderer, operation);
                })
        });
    }

    fn on_event(
        &mut self,
        tree: &mut widget::Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle<f32>,
    ) -> event::Status {
        let mut status = event::Status::Ignored;
        let state = tree.state.downcast_mut::<GraphContainerState>();
        let socket_state = self
            .socket_state
            .lock()
            .expect("should be able to lock socket state mutex in on_event()");

        // Socket-related processing
        if let Event::Mouse(mouse_event) = event {
            if let Some(cursor_position) = cursor.position_in(layout.bounds()) {
                let offset = self.matrix.get_translation();
                let translated_cursor_position =
                    Point::new(cursor_position.x - offset.0, cursor_position.y - offset.1);

                let scale = self.matrix.get_scale();
                let translated_descaled_cursor_position = Point::new(
                    translated_cursor_position.x / scale,
                    translated_cursor_position.y / scale,
                );

                // Find the socket we're hovering over
                let mut hovered_socket: Option<LogicalEndpoint> = None;
                for (role, node_sockets) in [
                    (SocketRole::In, &socket_state.inputs),
                    (SocketRole::Out, &socket_state.outputs),
                ] {
                    for (node_index, sockets) in node_sockets.iter().enumerate() {
                        for (socket_index, blob_rect) in sockets.iter().enumerate() {
                            if blob_rect.contains(translated_cursor_position) {
                                hovered_socket = Some(LogicalEndpoint {
                                    node_index,
                                    role,
                                    socket_index,
                                });
                            }
                        }
                    }
                }

                match mouse_event {
                    mouse::Event::ButtonPressed(mouse::Button::Left) => {
                        if let Some(hovered_socket) = hovered_socket {
                            match hovered_socket.role {
                                SocketRole::In => {
                                    // The primary intent of dragging from an input socket is
                                    // removing the connection to the previous node.
                                    // The crate user may still desire to implement a Blender-like
                                    // behaviour where it drags out a new connection
                                    if let Some(f) = &self.on_disconnect {
                                        shell.publish(f(
                                            hovered_socket,
                                            translated_descaled_cursor_position,
                                        ));
                                    }
                                }
                                SocketRole::Out => {
                                    // Create a new dangling connection from the output socket
                                    self.try_emit_dangling(
                                        shell,
                                        translated_descaled_cursor_position,
                                        hovered_socket,
                                    );
                                }
                            }
                            status = event::Status::Captured;
                        }
                    }
                    mouse::Event::CursorMoved { .. } => {
                        // Update the existing dangling connection, if it exists
                        if let Some(dangling_source) = self.dangling_source {
                            self.try_emit_dangling(
                                shell,
                                translated_descaled_cursor_position,
                                dangling_source,
                            );
                            status = event::Status::Captured;
                        }
                    }
                    mouse::Event::ButtonReleased(mouse::Button::Left) => {
                        if let Some(dangling_source) = self.dangling_source {
                            // No matter what happens, the dangling connection needs to be removed
                            if let Some(f) = &self.on_dangling {
                                shell.publish(f(None));
                            }

                            // If we're hovering over a socket while releasing the button,
                            // there's a chance we're about to make a connection
                            if let Some(hovered_socket) = hovered_socket {
                                // Don't allow connecting input to input or output to output
                                // sockets, and don't allow connecting a node to itself.
                                // This does not definitively detect cycles, but it's a start
                                if dangling_source.role != hovered_socket.role
                                    && dangling_source.node_index != hovered_socket.node_index
                                {
                                    if let Some(f) = &self.on_connect {
                                        let link = Link::from_unordered(
                                            Endpoint::Socket(dangling_source),
                                            Endpoint::Socket(hovered_socket),
                                        );
                                        shell.publish(f(link));
                                    }
                                }
                            }
                            status = event::Status::Captured;
                        }
                    }
                    _ => {}
                }
            }
        }

        if status == event::Status::Captured {
            return status;
        }

        if let Some(start) = state.drag_start_position {
            // Moving the viewport
            if let Some(cursor_position) = cursor.position_in(layout.bounds()) {
                match event {
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                        state.drag_start_position = None;
                    }
                    Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                        let delta = cursor_position - start;
                        state.drag_start_position = Some(cursor_position);
                        if let Some(f) = &self.on_translate {
                            let message = f((delta.x, delta.y));
                            shell.publish(message);
                        }
                        status = event::Status::Captured;
                    }
                    _ => {}
                }
            }
        } else {
            // Process events for our children (i.e. nodes), until one of the children
            // captures the event.
            // We process these in reverse storage order, as they are drawn in forward order,
            // and the last element in `content` is drawn on top.
            // So to match the intuitive expectation that events for the topmost node are processed
            // first, such that for example dragging a stack of nodes will only move the topmost
            // one, we need to reverse the direction.
            let event_queue: VecDeque<_> = self
                .content
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .collect();
            for ((child, state), layout) in event_queue.into_iter().rev() {
                let child_status = child.as_widget_mut().on_event(
                    state,
                    event.clone(),
                    layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                );
                status = status.merge(child_status);
                if status == event::Status::Captured {
                    break;
                }
            }
        }

        if status == event::Status::Ignored {
            if let Some(cursor_position) = cursor.position_in(layout.bounds()) {
                // Initiating viewport movement/scaling
                match event {
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                        state.drag_start_position = Some(cursor_position);
                        status = event::Status::Captured;
                    }
                    Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                        if let Some(f) = &self.on_scale {
                            match delta {
                                mouse::ScrollDelta::Lines { y, .. } => {
                                    let message = f(cursor_position.x, cursor_position.y, y);
                                    shell.publish(message);
                                }
                                mouse::ScrollDelta::Pixels { y, .. } => {
                                    let message = f(cursor_position.x, cursor_position.y, y);
                                    shell.publish(message);
                                }
                            }
                            status = event::Status::Captured;
                        }
                    }
                    _ => {}
                }
            }
        }

        status
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, state), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        state: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        renderer_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let style = theme.appearance(&self.style);

        let bounds = layout.bounds();

        renderer.with_layer(bounds, |renderer| {
            draw_background(renderer, bounds, style);

            let offset = self.matrix.get_translation();
            let scale = self.matrix.get_scale();
            let normalized_scale = normalize_scale(scale);

            let biggest_spacing = style
                .minor_guidelines_spacing
                .unwrap()
                .max(style.major_guidelines_spacing.unwrap())
                .max(style.mid_guidelines_spacing.unwrap());

            draw_guidelines(
                renderer,
                bounds,
                offset,
                normalized_scale,
                style.minor_guidelines_spacing.unwrap(),
                biggest_spacing,
                style.minor_guidelines_color.unwrap(),
            );

            draw_guidelines(
                renderer,
                bounds,
                offset,
                normalized_scale,
                style.mid_guidelines_spacing.unwrap(),
                biggest_spacing,
                style.mid_guidelines_color.unwrap(),
            );

            draw_guidelines(
                renderer,
                bounds,
                offset,
                normalized_scale,
                style.major_guidelines_spacing.unwrap(),
                biggest_spacing,
                style.major_guidelines_color.unwrap(),
            );

            let mut children_layout = layout.children();
            for i in 0..self.content.len() {
                let layout = children_layout.next().unwrap();
                let node = self.content[i].as_widget();

                let child_bounds = layout.bounds();
                let intersect = child_bounds.intersection(&bounds);

                if intersect.is_none() {
                    continue;
                }

                let intersect = intersect.unwrap();

                if intersect.width < 1.0 || intersect.height < 1.0 {
                    continue;
                }

                node.draw(
                    &state.children[i],
                    renderer,
                    theme,
                    renderer_style,
                    layout,
                    cursor,
                    viewport,
                );
            }
        });
    }
}

impl<'a, Message, Theme, Renderer> From<GraphContainer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: StyleSheet + 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(graph_container: GraphContainer<'a, Message, Theme, Renderer>) -> Self {
        Self::new(graph_container)
    }
}

fn draw_background<Renderer>(renderer: &mut Renderer, bounds: Rectangle, style: Appearance)
where
    Renderer: renderer::Renderer,
{
    renderer.fill_quad(
        renderer::Quad {
            bounds,
            border: Border {
                color: Color::BLACK,
                width: 0.0_f32,
                radius: [0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32].into(),
            },
            ..renderer::Quad::default()
        },
        style
            .background
            .unwrap_or(Background::Color(Color::from_rgb8(44, 44, 44))),
    );
}

fn draw_guidelines<Renderer>(
    renderer: &mut Renderer,
    bounds: Rectangle,
    offset: (f32, f32),
    scale: f32,
    grid_spacing: f32,
    biggest_grid_spacing: f32,
    color: Color,
) where
    Renderer: renderer::Renderer,
{
    if grid_spacing * scale < 5.0_f32 {
        return;
    }

    let edge = biggest_grid_spacing * scale;

    let offset_x = offset.0 % edge;
    let offset_y = offset.1 % edge;

    let from_x = -edge + offset_x + bounds.x;
    let to_x = bounds.x + bounds.width + edge;
    let step = grid_spacing * scale;
    let number_of_steps = ((to_x - from_x) / step).abs().ceil() as usize;

    for x in 0..number_of_steps {
        let x = from_x + (x as f32 * step);

        if x <= bounds.x || x >= bounds.x + bounds.width {
            continue;
        }

        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x,
                    y: bounds.y,
                    width: 1.0_f32,
                    height: bounds.height,
                },
                border: Border {
                    color: Color::BLACK,
                    width: 0.0_f32,
                    radius: [0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32].into(),
                },
                ..renderer::Quad::default()
            },
            Background::Color(color),
        );
    }

    let from_y = -edge + offset_y + bounds.y;
    let to_y = bounds.y + bounds.height + edge;
    let step = grid_spacing * scale;
    let number_of_steps = ((to_y - from_y) / step).abs().ceil() as usize;

    for y in 0..number_of_steps {
        let y = from_y + (y as f32 * step);

        if y <= bounds.y || y >= bounds.y + bounds.height {
            continue;
        }

        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x,
                    y,
                    width: bounds.width,
                    height: 1.0_f32,
                },
                border: Border {
                    color: Color::BLACK,
                    width: 0.0_f32,
                    radius: [0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32].into(),
                },
                ..renderer::Quad::default()
            },
            Background::Color(color),
        );
    }
}

fn normalize_scale(scale: f32) -> f32 {
    let log_2 = scale.log2().floor();

    if log_2.abs() > f32::EPSILON {
        scale / 2.0_f32.powf(log_2)
    } else {
        scale
    }
}
