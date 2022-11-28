use ethereum_types::U256;
use plonky2::field::types::Field;

use crate::cpu::columns::CpuColumnsView;
use crate::cpu::kernel::aggregator::KERNEL;
use crate::cpu::membus::NUM_GP_CHANNELS;
use crate::cpu::simple_logic::eq_iszero::generate_pinv_diff;
use crate::logic;
use crate::memory::segments::Segment;
use crate::witness::errors::ProgramError;
use crate::witness::memory::MemoryState;
use crate::witness::state::RegistersState;
use crate::witness::traces::Traces;
use crate::witness::util::{
    mem_read_gp_with_log_and_fill, mem_write_gp_log_and_fill, stack_pop_with_log_and_fill,
    stack_push_log_and_fill,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Dup(u8),
    Swap(u8),
    Iszero,
    Not,
    Syscall(u8),
    Eq,
    ExitKernel,
    BinaryLogic(BinaryLogicOp),
    NotImplemented,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryLogicOp {
    And,
    Or,
    Xor,
}

impl BinaryLogicOp {
    pub(self) fn result(&self, a: U256, b: U256) -> U256 {
        match self {
            BinaryLogicOp::And => a & b,
            BinaryLogicOp::Or => a | b,
            BinaryLogicOp::Xor => a ^ b,
        }
    }
}

fn make_logic_row<F: Field>(
    op: BinaryLogicOp,
    in0: U256,
    in1: U256,
    result: U256,
) -> [F; logic::columns::NUM_COLUMNS] {
    let mut row = [F::ZERO; logic::columns::NUM_COLUMNS];
    row[match op {
        BinaryLogicOp::And => logic::columns::IS_AND,
        BinaryLogicOp::Or => logic::columns::IS_OR,
        BinaryLogicOp::Xor => logic::columns::IS_XOR,
    }] = F::ONE;
    for i in 0..256 {
        row[logic::columns::INPUT0.start + i] = F::from_bool(in0.bit(i));
        row[logic::columns::INPUT1.start + i] = F::from_bool(in1.bit(i));
    }
    let result_limbs: &[u64] = result.as_ref();
    for (i, &limb) in result_limbs.iter().enumerate() {
        row[logic::columns::RESULT.start + 2 * i] = F::from_canonical_u32(limb as u32);
        row[logic::columns::RESULT.start + 2 * i + 1] = F::from_canonical_u32((limb >> 32) as u32);
    }
    row
}

pub fn generate_binary_logic_op<F: Field>(
    op: BinaryLogicOp,
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let [(in0, log_in0), (in1, log_in1)] =
        stack_pop_with_log_and_fill::<2, _>(&mut registers_state, memory_state, traces, &mut row)?;
    let result = op.result(in0, in1);
    let log_out = stack_push_log_and_fill(&mut registers_state, traces, &mut row, result)?;

    traces.push_logic(make_logic_row(op, in0, in1, result));
    traces.push_memory(log_in0);
    traces.push_memory(log_in1);
    traces.push_memory(log_out);
    traces.push_cpu(row);
    Ok(registers_state)
}

pub fn generate_dup<F: Field>(
    n: u8,
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let other_addr_lo = registers_state
        .stack_len
        .checked_sub(1 + (n as u32))
        .ok_or(ProgramError::StackUnderflow)?;
    let other_addr = (
        registers_state.context,
        Segment::Stack as u32,
        other_addr_lo,
    );

    let (val, log_in) =
        mem_read_gp_with_log_and_fill(0, other_addr, memory_state, traces, &mut row);
    let log_out = stack_push_log_and_fill(&mut registers_state, traces, &mut row, val)?;

    traces.push_memory(log_in);
    traces.push_memory(log_out);
    traces.push_cpu(row);
    Ok(registers_state)
}

pub fn generate_swap<F: Field>(
    n: u8,
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let other_addr_lo = registers_state
        .stack_len
        .checked_sub(2 + (n as u32))
        .ok_or(ProgramError::StackUnderflow)?;
    let other_addr = (
        registers_state.context,
        Segment::Stack as u32,
        other_addr_lo,
    );

    let [(in0, log_in0)] =
        stack_pop_with_log_and_fill::<1, _>(&mut registers_state, memory_state, traces, &mut row)?;
    let (in1, log_in1) =
        mem_read_gp_with_log_and_fill(1, other_addr, memory_state, traces, &mut row);
    let log_out0 =
        mem_write_gp_log_and_fill(NUM_GP_CHANNELS - 2, other_addr, traces, &mut row, in0);
    let log_out1 = stack_push_log_and_fill(&mut registers_state, traces, &mut row, in1)?;

    traces.push_memory(log_in0);
    traces.push_memory(log_in1);
    traces.push_memory(log_out0);
    traces.push_memory(log_out1);
    traces.push_cpu(row);
    Ok(registers_state)
}

pub fn generate_not<F: Field>(
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let [(x, log_in)] =
        stack_pop_with_log_and_fill::<1, _>(&mut registers_state, memory_state, traces, &mut row)?;
    let result = !x;
    let log_out = stack_push_log_and_fill(&mut registers_state, traces, &mut row, result)?;

    traces.push_memory(log_in);
    traces.push_memory(log_out);
    traces.push_cpu(row);
    Ok(registers_state)
}

pub fn generate_iszero<F: Field>(
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let [(x, log_in)] =
        stack_pop_with_log_and_fill::<1, _>(&mut registers_state, memory_state, traces, &mut row)?;
    let is_zero = x.is_zero();
    let result = {
        let t: u64 = is_zero.into();
        t.into()
    };
    let log_out = stack_push_log_and_fill(&mut registers_state, traces, &mut row, result)?;

    generate_pinv_diff(x, U256::zero(), &mut row);

    traces.push_memory(log_in);
    traces.push_memory(log_out);
    traces.push_cpu(row);
    Ok(registers_state)
}

pub fn generate_syscall<F: Field>(
    opcode: u8,
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let handler_jumptable_addr = KERNEL.global_labels["syscall_jumptable"] as u32;
    let handler_addr_addr = handler_jumptable_addr + (opcode as u32);
    let (handler_addr0, log_in0) = mem_read_gp_with_log_and_fill(
        0,
        (0, Segment::Code as u32, handler_addr_addr),
        memory_state,
        traces,
        &mut row,
    );
    let (handler_addr1, log_in1) = mem_read_gp_with_log_and_fill(
        1,
        (0, Segment::Code as u32, handler_addr_addr + 1),
        memory_state,
        traces,
        &mut row,
    );
    let (handler_addr2, log_in2) = mem_read_gp_with_log_and_fill(
        2,
        (0, Segment::Code as u32, handler_addr_addr + 2),
        memory_state,
        traces,
        &mut row,
    );

    let handler_addr = (handler_addr0 << 16) + (handler_addr1 << 8) + handler_addr2;
    let new_program_counter = handler_addr.as_u32();

    let syscall_info = U256::from(registers_state.program_counter)
        + (U256::from(u64::from(registers_state.is_kernel)) << 32);
    let log_out = stack_push_log_and_fill(&mut registers_state, traces, &mut row, syscall_info)?;

    registers_state.program_counter = new_program_counter;
    registers_state.is_kernel = true;

    traces.push_memory(log_in0);
    traces.push_memory(log_in1);
    traces.push_memory(log_in2);
    traces.push_memory(log_out);
    traces.push_cpu(row);

    Ok(registers_state)
}

pub fn generate_eq<F: Field>(
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let [(in0, log_in0), (in1, log_in1)] =
        stack_pop_with_log_and_fill::<2, _>(&mut registers_state, memory_state, traces, &mut row)?;
    let eq = in0 == in1;
    let result = U256::from(u64::from(eq));
    let log_out = stack_push_log_and_fill(&mut registers_state, traces, &mut row, result)?;

    generate_pinv_diff(in0, in1, &mut row);

    traces.push_memory(log_in0);
    traces.push_memory(log_in1);
    traces.push_memory(log_out);
    traces.push_cpu(row);
    Ok(registers_state)
}

pub fn generate_exit_kernel<F: Field>(
    mut registers_state: RegistersState,
    memory_state: &MemoryState,
    traces: &mut Traces<F>,
    mut row: CpuColumnsView<F>,
) -> Result<RegistersState, ProgramError> {
    let [(kexit_info, log_in)] =
        stack_pop_with_log_and_fill::<1, _>(&mut registers_state, memory_state, traces, &mut row)?;
    let kexit_info_u64: [u64; 4] = kexit_info.0;
    let program_counter = kexit_info_u64[0] as u32;
    let is_kernel_mode_val = (kexit_info_u64[1] >> 32) as u32;
    assert!(is_kernel_mode_val == 0 || is_kernel_mode_val == 1);
    let is_kernel_mode = is_kernel_mode_val != 0;

    registers_state.program_counter = program_counter;
    registers_state.is_kernel = is_kernel_mode;

    traces.push_memory(log_in);
    traces.push_cpu(row);

    Ok(registers_state)
}
