use minijinja::compiler::cfg::build_cfg;
use minijinja::compiler::instructions::Instruction;

#[cfg(test)]
#[test]
fn simple_cfg() {
    use minijinja::machinery::Span;

    let code = vec![
        Instruction::LoadConst(1.into()),
        Instruction::JumpIfFalse(4, Span::default()),
        Instruction::LoadConst(2.into()),
        Instruction::Jump(5, Span::default()),
        Instruction::LoadConst(3.into()),
        Instruction::Return {
            explicit: false,
            arg_count: None,
        },
    ];

    let cfg = build_cfg(&code);

    assert!(cfg.blocks.len() == 4);
}
