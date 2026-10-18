' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 10
Function SumAll(ParamArray nums() As Variant) As Double
    Dim v As Variant
    For Each v In nums
        SumAll = SumAll + v
    Next v
End Function
Sub Main()
    Debug.Print SumAll(1, 2, 3, 4)
End Sub
