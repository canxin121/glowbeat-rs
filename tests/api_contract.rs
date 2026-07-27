use glowbeat::{EngineConfig, LightPoint, Rgba, StreamingEngine};

#[test]
fn capacity_and_length_errors_are_reported() {
    let mut engine = StreamingEngine::new(EngineConfig::default(), 4).unwrap();
    let points = LightPoint::line(5);
    let mut output = vec![Rgba::default(); 5];
    assert!(engine.render_now(&points, &mut output).is_err());

    let points = LightPoint::line(4);
    let mut wrong_output = vec![Rgba::default(); 3];
    assert!(engine.render_now(&points, &mut wrong_output).is_err());
}
